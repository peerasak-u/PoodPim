"""Send Function-URL-shaped events to a local Lambda container (Runtime Interface Emulator).

  docker run --rm -p 9000:8080 -e POODPIM_API_KEY=test thocr-asr:dev
  python3 deploy/lambda/invoke_local.py test warm
  python3 deploy/lambda/invoke_local.py test transcribe tests/audio/th1.wav
"""

import base64
import json
import sys
import time
import urllib.request

RIE = "http://localhost:9000/2015-03-31/functions/function/invocations"


def event(path: str, key: str, body: bytes | None = None, query: str = "") -> dict:
    return {
        "version": "2.0",
        "routeKey": "$default",
        "rawPath": path,
        "rawQueryString": query,
        "queryStringParameters": dict(p.split("=", 1) for p in query.split("&") if "=" in p) or None,
        "headers": {"x-api-key": key, "content-type": "audio/wav" if body else "application/json", "host": "local"},
        "requestContext": {
            "accountId": "anonymous",
            "apiId": "local",
            "domainName": "local",
            "domainPrefix": "local",
            "http": {"method": "POST", "path": path, "protocol": "HTTP/1.1", "sourceIp": "127.0.0.1", "userAgent": "test"},
            "requestId": "local",
            "routeKey": "$default",
            "stage": "$default",
            "time": "01/Jan/2026:00:00:00 +0000",
            "timeEpoch": 0,
        },
        "body": base64.b64encode(body).decode() if body else None,
        "isBase64Encoded": bool(body),
    }


def call(ev: dict) -> dict:
    req = urllib.request.Request(RIE, data=json.dumps(ev).encode(), headers={"content-type": "application/json"})
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=300) as r:
        out = json.load(r)
    out["_client_s"] = round(time.time() - t0, 2)
    return out


if __name__ == "__main__":
    key, action = sys.argv[1], sys.argv[2]
    if action == "warm":
        print(call(event("/warm", key)))
    else:
        for f in sys.argv[3:]:
            print(f, call(event("/transcribe", key, open(f, "rb").read(), "mixed=1")))
