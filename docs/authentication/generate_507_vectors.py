#!/usr/bin/env python3
"""公開の使い捨て鍵。Python 標準 hmac/hashlib で Sigil と独立に固定値を再計算する。"""
import base64
import hashlib
import hmac
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
KEY = bytes(range(32))
NOW = 1700000000
AUD = b"sigil.api"
SUB = b"user:alice-01"
MAX_TIME = 253402300799
ROWS = []


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=")


def payload(iat=NOW, exp=NOW + 3600, aud=AUD, sub=SUB):
    return struct.pack(">QQH", iat, exp, len(aud)) + aud + struct.pack(">H", len(sub)) + sub


def row(name, data=None, *, key=KEY, signing_key=None, audience=AUD.decode(), now=NOW,
        result="ERR", transform=None, message=None, tag_transform=None):
    if data is None:
        data = payload()
    signed = b"sigil1." + b64(data)
    actual_key = signing_key or key
    actual_message = signed if message is None else message
    computed_tag = hmac.new(actual_key, actual_message, hashlib.sha256).digest()
    tag = computed_tag
    if tag_transform:
        tag = tag_transform(tag)
    token = signed + b"." + b64(tag)
    if transform:
        token = transform(token)
    # 署名対象・payload・鍵・タグは省略せず全バイトを記録する。
    ROWS.append([name, key.hex(), data.hex(), signed.hex(), tag.hex(), b64(tag).decode(),
                 token.hex(), audience, str(now), result, actual_key.hex(), actual_message.hex(), computed_tag.hex()])


row("valid_iat_equals_now_lifetime_3600", result=SUB.decode())
row("valid_last_second", now=NOW + 3599, result=SUB.decode())
row("valid_lifetime_1", payload(exp=NOW + 1), result=SUB.decode())
row("valid_zero_iat", payload(iat=0, exp=1), now=0, result=SUB.decode())
row("valid_max_time", payload(iat=MAX_TIME - 1, exp=MAX_TIME), now=MAX_TIME - 1, result=SUB.decode())
row("valid_identifier_alphabet", payload(aud=b"AZaz09._:-", sub=b"AZaz09._:-"), audience="AZaz09._:-", result="AZaz09._:-")
row("valid_max_payload_404_token_590", payload(aud=b"a" * 128, sub=b"s" * 256), audience="a" * 128, result="s" * 256)
row("expired_equals_now", now=NOW + 3600)
row("expired_after_now", now=NOW + 3601)
row("future_iat", now=NOW - 1)
row("lifetime_3601", payload(exp=NOW + 3601))
row("zero_lifetime", payload(exp=NOW))
row("reverse_lifetime", payload(exp=NOW - 1))
row("iat_out_of_range", payload(iat=MAX_TIME + 1, exp=MAX_TIME + 2), now=MAX_TIME)
row("exp_out_of_range", payload(iat=MAX_TIME - 1, exp=MAX_TIME + 1), now=MAX_TIME - 1)
row("iat_u64_max", payload(iat=2**64 - 1))
row("exp_u64_max", payload(exp=2**64 - 1))
row("iat_signed_overflow", payload(iat=2**63))
row("exp_signed_overflow", payload(exp=2**63))
row("now_negative", now=-1)
row("now_out_of_range", now=MAX_TIME + 1)
row("audience_mismatch", audience="sigil.other")
row("audience_case_sensitive", audience="Sigil.api")
row("empty_config_audience", audience="")
row("invalid_config_audience", audience="sigil/api")
row("long_config_audience", audience="a" * 129)
row("empty_audience", payload(aud=b""))
row("long_audience_129", payload(aud=b"a" * 129), audience="a" * 128)
row("invalid_audience", payload(aud=b"sigil/api"))
row("nonascii_audience", payload(aud=b"sigil\xff"))
row("empty_subject", payload(sub=b""))
row("long_subject_257", payload(sub=b"s" * 257))
row("invalid_subject_space", payload(sub=b"user alice"))
row("invalid_subject_slash", payload(sub=b"user/alice"))
row("nonascii_subject", payload(sub=b"user\xff"))
row("nul_subject", payload(sub=b"user\x00"))
row("trailing_payload_byte", payload() + b"\0")
row("signed_short_payload", b"\xff")
row("wrong_tag_short_payload", b"\xff", tag_transform=lambda t: bytes([t[0] ^ 1]) + t[1:])
row("truncated_aud_length", payload()[:17])
row("truncated_audience", payload()[:22])
row("truncated_subject_length", payload()[:18 + len(AUD) + 1])
row("truncated_subject", payload()[:-1])
row("aud_length_65535", payload()[:16] + b"\xff\xff" + payload()[18:])
row("sub_length_65535", payload()[:18 + len(AUD)] + b"\xff\xff" + SUB)
row("payload_405", payload(aud=b"a" * 128, sub=b"s" * 256) + b"s", audience="a" * 128)
row("key_31", key=KEY[:-1])
row("key_33", key=KEY + b"\x20")
row("wrong_key", key=b"x" * 32, signing_key=KEY)
row("signed_decoded_payload_instead_of_received_ascii", message=payload())
row("signed_without_version", message=b64(payload()))
row("signed_with_tag_separator", message=b"sigil1." + b64(payload()) + b".")
row("empty_token", transform=lambda t: b"")
row("unknown_version", transform=lambda t: t.replace(b"sigil1", b"sigil2"))
row("extra_separator", transform=lambda t: t + b".")
row("missing_separator", transform=lambda t: t.replace(b".", b"", 1))
row("empty_payload", transform=lambda t: b"sigil1.." + t.rsplit(b".", 1)[1])
row("empty_tag", transform=lambda t: t.rsplit(b".", 1)[0] + b".")
row("truncated_tag", tag_transform=lambda t: t[:-1])
row("long_tag", tag_transform=lambda t: t + b"\0")
row("payload_padding", transform=lambda t: t.rsplit(b".", 1)[0] + b"=." + t.rsplit(b".", 1)[1])
row("tag_padding", transform=lambda t: t + b"=")
row("standard_plus", b"\xfb", transform=lambda t: t.replace(b"-", b"+"))
row("standard_slash", b"\xff", transform=lambda t: t.replace(b"_", b"/"))
row("payload_impossible_length", transform=lambda t: b"sigil1.A." + t.rsplit(b".", 1)[1])
row("payload_nonzero_unused_4_bits", b"\xff", transform=lambda t: t.replace(b"_w.", b"_x."))
row("payload_nonzero_unused_2_bits", b"\xff\xff", transform=lambda t: t.replace(b"__8.", b"__9."))
row("tag_nonzero_unused_bits", transform=lambda t: t[:-1] + bytes([t[-1] + 1]))
row("trailing_space", transform=lambda t: t + b" ")
row("leading_space", transform=lambda t: b" " + t)
row("tab", transform=lambda t: t[:8] + b"\t" + t[9:])
row("nul", transform=lambda t: t[:8] + b"\0" + t[9:])
row("nonascii_token", transform=lambda t: t[:8] + b"\xff" + t[9:])
row("token_1024", transform=lambda t: t + b"a" * (1024 - len(t)))
row("token_1025", transform=lambda t: t + b"a" * (1025 - len(t)))
for i in range(32):
    row(f"tag_corruption_{i:02}", tag_transform=lambda t, i=i: t[:i] + bytes([t[i] ^ 1]) + t[i+1:])

header = "# id\tkey_hex\tpayload_hex\tsigned_ascii_hex\ttag_hex\ttag_base64url\treceived_token_hex\taudience\tnow\tresult\tsigning_key_hex\thmac_input_hex\tcomputed_tag_hex\n"
(ROOT / "tests/data/bearer_507.tsv").write_text(header + "".join("\t".join(r) + "\n" for r in ROWS))
print(f"{len(ROWS)} public vectors")
