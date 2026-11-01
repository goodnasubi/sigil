use super::*;

mod typed_tests {
    use super::*;

    /// 式の番地を集める。呼び出しの名前空間・型の接頭辞（`fs`・`Path`）は値ではないので数えない。
    fn walk_expr(e: &Expr, t: &Typed<'_>, out: &mut Vec<(usize, String)>) {
        out.push((addr(e), format!("{e:?}")));
        match e {
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Bool(_)
            | Expr::Name(_)
            | Expr::Result
            | Expr::SelfValue => {}
            Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => walk_expr(x, t, out),
            Expr::Unary { expr, .. } => walk_expr(expr, t, out),
            Expr::Tuple(xs) => xs.iter().for_each(|x| walk_expr(x, t, out)),
            Expr::Binary { lhs, rhs, .. } => {
                walk_expr(lhs, t, out);
                walk_expr(rhs, t, out);
            }
            Expr::Field { expr, .. } => {
                // `ConfigError.NotFound`・`fs.read` の前置きは型・名前空間の名前で、値ではない
                let ns = matches!(&**expr, Expr::Name(n)
                    if n == "fs" || n == "console" || n.contains('.') || n.starts_with(|c: char| c.is_uppercase()));
                if !ns {
                    walk_expr(expr, t, out)
                }
            }
            Expr::Call { callee, args } => {
                let prefix_only = matches!(
                    t.calls.get(&addr(e)).map(|r| r.kind),
                    Some(ResKind::Namespaced | ResKind::TypeFn | ResKind::Variant | ResKind::Check)
                );
                match (&**callee, prefix_only) {
                    (Expr::Field { .. }, true) => out.push((addr(callee), format!("{callee:?}"))),
                    _ => walk_expr(callee, t, out),
                }
                args.iter().for_each(|x| walk_expr(x, t, out));
            }
            Expr::StructLit { fields, .. } => fields.iter().for_each(|(_, x)| walk_expr(x, t, out)),
            Expr::Match {
                scrutinee, arms, ..
            } => {
                walk_expr(scrutinee, t, out);
                arms.iter().for_each(|a| walk_expr(&a.body, t, out));
            }
            Expr::Closure { body, .. } => match body {
                ClosureBody::Expr(x) => walk_expr(x, t, out),
                ClosureBody::Block(b) => walk_block(b, t, out),
            },
        }
    }

    fn walk_block(b: &Block, t: &Typed<'_>, out: &mut Vec<(usize, String)>) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let { value, .. } => walk_expr(value, t, out),
                StmtKind::Assign { target, value } => {
                    walk_expr(target, t, out);
                    walk_expr(value, t, out);
                }
                StmtKind::Expr(e) => walk_expr(e, t, out),
                StmtKind::Return(e) => {
                    if let Some(e) = e {
                        walk_expr(e, t, out)
                    }
                }
                StmtKind::If(i) => walk_if(i, t, out),
                StmtKind::While { cond, body } => {
                    walk_expr(cond, t, out);
                    walk_block(body, t, out);
                }
                StmtKind::Match {
                    scrutinee, arms, ..
                } => {
                    walk_expr(scrutinee, t, out);
                    arms.iter().for_each(|a| walk_block(&a.body, t, out));
                }
            }
        }
    }

    fn walk_if(i: &IfStmt, t: &Typed<'_>, out: &mut Vec<(usize, String)>) {
        walk_expr(&i.cond, t, out);
        walk_block(&i.then, t, out);
        match i.otherwise.as_deref() {
            None => {}
            Some(Else::If(n)) => walk_if(n, t, out),
            Some(Else::Block(b)) => walk_block(b, t, out),
        }
    }

    fn walk_fn(f: &FnDecl, t: &Typed<'_>, out: &mut Vec<(usize, String)>) {
        if let Some(r) = &f.requires {
            walk_expr(r, t, out);
        }
        if let Some(e) = &f.ensures {
            walk_expr(e, t, out);
        }
        walk_block(&f.body, t, out);
    }

    fn lets(b: &Block, out: &mut Vec<usize>) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let { .. } => out.push(s as *const Stmt as usize),
                StmtKind::If(i) => lets_if(i, out),
                StmtKind::While { body, .. } => lets(body, out),
                StmtKind::Match { arms, .. } => arms.iter().for_each(|a| lets(&a.body, out)),
                _ => {}
            }
        }
    }

    fn lets_if(i: &IfStmt, out: &mut Vec<usize>) {
        lets(&i.then, out);
        match i.otherwise.as_deref() {
            None => {}
            Some(Else::If(n)) => lets_if(n, out),
            Some(Else::Block(b)) => lets(b, out),
        }
    }

    /// すべての式に `expr_ty` が、すべての `let` に `let_ty` がある。
    fn covers(label: &str, src: &str) {
        let file = parse(src).unwrap_or_else(|e| panic!("{label}: {e}"));
        let t = typed(&file).unwrap_or_else(|e| panic!("{label}: {e}"));
        let mut exprs = vec![];
        let mut let_stmts = vec![];
        for item in &file.items {
            match item {
                Item::Fn(f) => {
                    walk_fn(f, &t, &mut exprs);
                    lets(&f.body, &mut let_stmts);
                }
                Item::Impl(i) => {
                    for f in &i.fns {
                        walk_fn(f, &t, &mut exprs);
                        lets(&f.body, &mut let_stmts);
                    }
                }
                Item::Type(d) => walk_expr(&d.cond, &t, &mut exprs),
                Item::Struct(_) | Item::Enum(_) => {}
                Item::Global(g) => walk_expr(&g.init, &t, &mut exprs),
                // トレイトを含むソースは `typed` の段階で止まるので、ここには来ない。
                Item::Trait(_) | Item::ImplFor(_) => {}
            }
        }
        assert!(!exprs.is_empty(), "{label}: 式がない");
        for (a, dbg) in &exprs {
            assert!(t.expr_ty.contains_key(a), "{label}: 型の記録がない式 {dbg}");
        }
        for a in &let_stmts {
            assert!(t.let_ty.contains_key(a), "{label}: 型の記録がない let");
        }
        // 表の鍵は、どれも file の中の式・文の番地である
        for k in t.expr_ty.keys() {
            assert!(
                exprs.iter().any(|(a, _)| a == k),
                "{label}: file にない番地が記録されている"
            );
        }
    }

    const MIX: &str = r#"module m.mix
  targets server
  trust none

type Port = Int where self >= 0 && self <= 65535;

struct Pt {
  x: Int,
  y: Int,
}

enum Shape {
  Dot,
  Line(Int),
  Box(Int, Int),
}

impl Pt {
  fn sum(self: &Pt) -> Int
    intent "x と y の和"
    requires none
    ensures none
    effects none
  {
    return self.x + self.y;
  }
}

fn pair<T>(a: T, b: T) -> (T, T)
  intent "組にする"
  requires none
  ensures none
  effects none
{
  return (a, b);
}

fn area(s: &Shape) -> Int
  intent "面積"
  requires none
  ensures result >= 0 || result < 0
  effects none
{
  let r = match s {
    Shape.Dot => 0,
    Shape.Line(n) => n,
    Shape.Box(a, b) => a * b,
  };
  return r;
}

fn run(xs: &List<Int>) -> Int
  intent "いろいろな式"
  requires xs.len() >= 0
  ensures result == result
  effects none
{
  let p = Pt { x: 1, y: -2 };
  let q = pair(1, 2);
  let f = 1.5;
  let s = "a";
  let ok = !(1 < 2) || true;
  let port = Port.check(8080);
  let mut v = List.new();
  v.push(1);
  let ys = xs.map(|x| x + 1);
  let mut i = 0;
  while i < 3 {
    i = i + 1;
  }
  if ok {
    let w = p.sum();
    return w + i;
  } else if i > 1 {
    return 1;
  }
  match q {
    (a, b) => {
      return a + b;
    }
  }
}
"#;

    const TRY: &str = r#"module m.t
  targets server
  trust none

fn parse(s: &String) -> Result<Int, ParseError>
  intent "整数にする"
  requires none
  ensures none
  effects none
{
  let n = s.parse_int()?;
  return Ok(n + 1);
}

fn opt(o: Option<Int>) -> Int
  intent "既定値"
  requires none
  ensures none
  effects none
{
  let r = match o {
    Some(n) => n,
    None => 0,
  };
  return r;
}

fn io(p: Path) -> Int
  intent "読む"
  requires none
  ensures none
  effects console, fs.read
{
  console.log(&1);
  let r = fs.read(&p);
  let q = Path.new(&"a");
  let e = q.is_empty();
  return 0;
}
"#;

    #[test]
    fn typed_covers_every_expr() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for rel in [
            "examples/01-average/c-brace.sigil",
            "examples/02-read-port/c-brace.sigil",
            "examples/03-ownership/ownership.sigil",
            "examples/04-effects/effects.sigil",
            "examples/05-control/control.sigil",
        ] {
            let src = std::fs::read_to_string(root.join(rel)).unwrap();
            covers(rel, &src);
        }
        covers("MIX", MIX);
        covers("TRY", TRY);
    }

    #[test]
    fn typed_records_calls_moves_and_lets() {
        let src = r#"module m.r
  targets server
  trust none

fn pair<T>(a: T, b: T) -> (T, T)
  intent "組にする"
  requires none
  ensures none
  effects none
{
  return (a, b);
}

fn take(xs: List<Int>) -> Int
  intent "消費する"
  requires none
  ensures none
  effects none
{
  return xs.len();
}

fn main() -> Int
  intent "確かめ"
  requires none
  ensures none
  effects none
{
  let q = pair(1, 2);
  let mut v = List.new();
  v.push(1);
  let n = take(v);
  let k = 3;
  let m = k + k;
  return n + m;
}
"#;
        let file = parse(src).unwrap();
        let t = typed(&file).unwrap();
        let Item::Fn(main) = &file.items[2] else {
            panic!()
        };
        fn call_of(s: &Stmt) -> &Expr {
            match &s.kind {
                StmtKind::Let { value, .. } | StmtKind::Expr(value) => value,
                _ => panic!(),
            }
        }
        let st = &main.body.stmts;
        // 総称の関数: 型引数が具体化されている
        let r = &t.calls[&addr(call_of(&st[0]))];
        assert_eq!((r.kind, r.name.as_str()), (ResKind::Fn, "pair"));
        assert_eq!(r.targs, vec![("T".to_string(), Ty::Int)]);
        // 型に付く関数とメソッド
        let r = &t.calls[&addr(call_of(&st[1]))];
        assert_eq!(
            (r.kind, r.owner.as_deref(), r.name.as_str()),
            (ResKind::TypeFn, Some("List"), "new")
        );
        assert_eq!(r.targs, vec![("T".to_string(), Ty::Int)]);
        let r = &t.calls[&addr(call_of(&st[2]))];
        assert_eq!(
            (r.kind, r.owner.as_deref(), r.name.as_str()),
            (ResKind::Method, Some("List"), "push")
        );
        // `let` の型
        assert_eq!(
            t.let_ty[&(&st[0] as *const Stmt as usize)],
            Ty::Tuple(vec![Ty::Int, Ty::Int])
        );
        assert_eq!(t.let_ty[&(&st[4] as *const Stmt as usize)], Ty::Int);
        // 移動: `take(v)` の `v`。`k + k` の読みは複製
        let StmtKind::Let { value, .. } = &st[3].kind else {
            panic!()
        };
        let Expr::Call { args, .. } = value else {
            panic!()
        };
        assert!(t.moves.contains(&addr(&args[0])));
        let StmtKind::Let { value, .. } = &st[5].kind else {
            panic!()
        };
        let Expr::Binary { lhs, rhs, .. } = value else {
            panic!()
        };
        assert!(!t.moves.contains(&addr(lhs)) && !t.moves.contains(&addr(rhs)));
        // ほかの移動は `pair` の `(a, b)` の二つ(`T` は Copyable と限らない)
        assert_eq!(t.moves.len(), 3);
    }
}

mod std_heads_tests {
    #[test]
    fn every_stdlib_namespace_is_reserved() {
        for (_, ns, _, _) in crate::stdlib::NAMESPACED_FNS {
            assert!(
                super::std_heads().contains(ns),
                "{ns} は ADR 0021 の予約一覧にない"
            );
        }
    }
}
