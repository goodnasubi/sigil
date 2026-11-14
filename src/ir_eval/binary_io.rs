//! ADR 0036 の評価器側の独立したバイナリ I/O 境界。
use std::io::{self, Read, Write};

pub(super) const LIMIT: i64 = 134_217_728;

#[derive(Debug)]
pub(super) enum Error {
    InvalidLimit,
    InvalidPath,
    TooLarge,
    UnsupportedFile,
    Io(io::Error),
}

pub(super) fn validate(path: &str, max: i64) -> Result<usize, Error> {
    if !(0..=LIMIT).contains(&max) {
        return Err(Error::InvalidLimit);
    }
    if path.len() > 4095 || path.as_bytes().contains(&0) {
        return Err(Error::InvalidPath);
    }
    Ok(max as usize)
}

/// 短い I/O・EINTR・close 失敗とメタデータ競合を決定的に注入できる境界。
trait Io {
    fn metadata(&mut self) -> io::Result<(bool, u64)>;
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, src: &[u8]) -> io::Result<usize>;
    fn close(self) -> io::Result<()>;
}

fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match f() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            r => return r,
        }
    }
}

fn read_from(mut file: impl Io, max: usize) -> Result<Vec<u8>, Error> {
    let result = (|| {
        let (regular, len) = retry(|| file.metadata()).map_err(Error::Io)?;
        if !regular {
            return Err(Error::UnsupportedFile);
        }
        if len > max as u64 {
            return Err(Error::TooLarge);
        }
        // 本体は一回だけ max バイト。max=0 はヒープ確保を伴わない。
        let mut bytes = vec![0; max];
        let mut n = 0;
        while n < max {
            let r = retry(|| file.read(&mut bytes[n..])).map_err(Error::Io)?;
            if r == 0 {
                bytes.truncate(n);
                return Ok(bytes);
            }
            n += r;
        }
        let mut probe = [0; 1];
        if retry(|| file.read(&mut probe)).map_err(Error::Io)? != 0 {
            return Err(Error::TooLarge);
        }
        Ok(bytes)
    })();
    // close は一度だけ。先行エラーを優先し、prefix は返さない。
    let close = file.close().map_err(Error::Io);
    match result {
        Err(e) => Err(e),
        Ok(bytes) => close.map(|()| bytes),
    }
}

fn write_to(mut file: impl Io, bytes: &[u8]) -> Result<(), Error> {
    let result = (|| {
        let (regular, _) = retry(|| file.metadata()).map_err(Error::Io)?;
        if !regular {
            return Err(Error::UnsupportedFile);
        }
        let mut n = 0;
        while n < bytes.len() {
            let r = retry(|| file.write(&bytes[n..])).map_err(Error::Io)?;
            if r == 0 {
                return Err(Error::Io(io::ErrorKind::WriteZero.into()));
            }
            n += r;
        }
        Ok(())
    })();
    let close = file.close().map_err(Error::Io);
    result.and(close)
}

struct File(std::fs::File);
impl Io for File {
    fn metadata(&mut self) -> io::Result<(bool, u64)> {
        self.0.metadata().map(|m| (m.is_file(), m.len()))
    }
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        self.0.read(dst)
    }
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        self.0.write(src)
    }
    fn close(self) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::IntoRawFd;
            unsafe extern "C" {
                fn close(fd: std::ffi::c_int) -> std::ffi::c_int;
            }
            let fd = self.0.into_raw_fd();
            // fd の所有権を受け取り一回だけ close。EINTR でも再試行しない。
            if unsafe { close(fd) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::IntoRawHandle;
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
            }
            if unsafe { CloseHandle(self.0.into_raw_handle()) } != 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            drop(self);
            Ok(())
        }
    }
}

fn open(path: &str, write: bool) -> Result<File, Error> {
    let mut options = std::fs::OpenOptions::new();
    if write {
        options.write(true).create(true).truncate(true);
    } else {
        options.read(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const NONBLOCK: i32 = if cfg!(any(target_arch = "mips", target_arch = "mips64")) {
            0x80
        } else if cfg!(any(target_arch = "sparc", target_arch = "sparc64")) {
            0x4000
        } else {
            0x800
        };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const NONBLOCK: i32 = 4;
        options.custom_flags(NONBLOCK);
    }
    retry(|| options.open(path)).map(File).map_err(Error::Io)
}

pub(super) fn read(path: &str, max: usize) -> Result<Vec<u8>, Error> {
    read_from(open(path, false)?, max)
}

pub(super) fn write(path: &str, bytes: &[u8]) -> Result<(), Error> {
    write_to(open(path, true)?, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    struct Mock {
        bytes: Vec<u8>,
        at: usize,
        declared: u64,
        regular: bool,
        steps: VecDeque<io::Result<usize>>,
        close_error: bool,
        closed: Rc<RefCell<usize>>,
        requests: Rc<RefCell<Vec<usize>>>,
    }
    impl Mock {
        fn new(bytes: &[u8], declared: u64) -> Self {
            Self {
                bytes: bytes.to_vec(),
                at: 0,
                declared,
                regular: true,
                steps: VecDeque::new(),
                close_error: false,
                closed: Rc::default(),
                requests: Rc::default(),
            }
        }
    }
    impl Io for Mock {
        fn metadata(&mut self) -> io::Result<(bool, u64)> {
            Ok((self.regular, self.declared))
        }
        fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
            self.requests.borrow_mut().push(dst.len());
            let step = self.steps.pop_front().unwrap_or(Ok(dst.len()))?;
            let n = step.min(dst.len()).min(self.bytes.len() - self.at);
            dst[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
        fn write(&mut self, src: &[u8]) -> io::Result<usize> {
            self.requests.borrow_mut().push(src.len());
            let n = self
                .steps
                .pop_front()
                .unwrap_or(Ok(src.len()))?
                .min(src.len());
            self.bytes.extend_from_slice(&src[..n]);
            Ok(n)
        }
        fn close(self) -> io::Result<()> {
            *self.closed.borrow_mut() += 1;
            if self.close_error {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn growth_probe_shrink_and_preallocation_rejection() {
        let m = Mock::new(&[0, 255, 1], 1);
        let requests = m.requests.clone();
        let closed = m.closed.clone();
        assert!(matches!(read_from(m, 2), Err(Error::TooLarge)));
        assert_eq!(*requests.borrow(), vec![2, 1]);
        assert_eq!(*closed.borrow(), 1);
        assert_eq!(read_from(Mock::new(&[255], 2), 2).unwrap(), vec![255]);
        let m = Mock::new(&[1], 3);
        let requests = m.requests.clone();
        let closed = m.closed.clone();
        assert!(matches!(read_from(m, 2), Err(Error::TooLarge)));
        assert!(requests.borrow().is_empty());
        assert_eq!(*closed.borrow(), 1);
        assert!(read_from(Mock::new(&[], 0), 0).unwrap().is_empty());
        assert!(matches!(
            read_from(Mock::new(&[1], 0), 0),
            Err(Error::TooLarge)
        ));
    }
    #[test]
    fn short_io_and_interrupts_continue_at_same_offset() {
        let mut m = Mock::new(&[0, 255, 1], 3);
        m.steps = VecDeque::from([Err(io::ErrorKind::Interrupted.into()), Ok(1), Ok(1), Ok(1)]);
        let requests = m.requests.clone();
        assert_eq!(read_from(m, 3).unwrap(), vec![0, 255, 1]);
        assert_eq!(*requests.borrow(), vec![3, 3, 2, 1, 1]);
        let mut m = Mock::new(&[], 0);
        m.steps = VecDeque::from([Ok(1), Err(io::ErrorKind::Interrupted.into()), Ok(1)]);
        let requests = m.requests.clone();
        let closed = m.closed.clone();
        write_to(m, &[0, 255, 1]).unwrap();
        assert_eq!(*requests.borrow(), vec![3, 2, 2, 1]);
        assert_eq!(*closed.borrow(), 1);
    }
    #[test]
    fn errors_discard_prefix_and_close_once_without_retry() {
        let mut m = Mock::new(&[0, 255], 2);
        m.close_error = true;
        let closed = m.closed.clone();
        assert!(
            matches!(read_from(m,2),Err(Error::Io(e)) if e.kind()==io::ErrorKind::PermissionDenied)
        );
        assert_eq!(*closed.borrow(), 1);
        let mut m = Mock::new(&[0, 255], 2);
        m.close_error = true;
        m.steps = VecDeque::from([Ok(1), Err(io::ErrorKind::Other.into())]);
        assert!(matches!(read_from(m,2),Err(Error::Io(e)) if e.kind()==io::ErrorKind::Other));
        let mut m = Mock::new(&[], 0);
        m.close_error = true;
        m.steps = VecDeque::from([Ok(1), Ok(0)]);
        let closed = m.closed.clone();
        assert!(
            matches!(write_to(m,&[1,2]),Err(Error::Io(e)) if e.kind()==io::ErrorKind::WriteZero)
        );
        assert_eq!(*closed.borrow(), 1);
        let mut m = Mock::new(&[], 0);
        m.close_error = true;
        assert!(matches!(write_to(m, &[]), Err(Error::Io(_))));
    }
    #[test]
    fn validation_order_and_integer_extremes() {
        for n in [i64::MIN, -1, LIMIT + 1, i64::MAX] {
            assert!(matches!(validate("\0", n), Err(Error::InvalidLimit)));
        }
        for n in [0, 1, LIMIT] {
            assert_eq!(validate("ok", n).unwrap(), n as usize);
        }
        assert!(matches!(
            validate(&"a".repeat(4096), 1),
            Err(Error::InvalidPath)
        ));
        assert!(matches!(validate("x\0", 0), Err(Error::InvalidPath)));
    }
}
