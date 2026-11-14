"""試験だけの独立暗号オラクル。通信・接続・プロトコル状態は実装しない。"""
import base64
import hashlib
import hmac
import sys

salted = hashlib.pbkdf2_hmac("sha256", b"pencil", base64.b64decode("W22ZaJ0SNY7soEsUEjb6gQ=="), 4096)
auth = sys.argv[1].encode("ascii")
client_key = hmac.digest(salted, b"Client Key", "sha256")
client_sig = hmac.digest(hashlib.sha256(client_key).digest(), auth, "sha256")
proof = bytes(a ^ b for a, b in zip(client_key, client_sig))
server_key = hmac.digest(salted, b"Server Key", "sha256")
print(base64.b64encode(proof).decode())
print(base64.b64encode(hmac.digest(server_key, auth, "sha256")).decode())
