#!/usr/bin/env bash
# List this gateway's registered workspaces and reported health.
# Usage: list-workspaces.sh <gateway-origin> <local-token-file> [--ca-file <pem>]
# For a private HTTPS certificate, pass its trusted CA PEM with --ca-file.
# Reads GET /~/api/v1/control/workspaces. Ports files are not health signals.

set -euo pipefail

exec python3 - "$@" <<'PY'
import argparse
import json
import http.client
from pathlib import Path
import sys
import ssl
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


parser = argparse.ArgumentParser(description="List gateway workspace health without guessing from ports files.")
parser.add_argument("gateway_origin", help="Local gateway origin, including http:// or https:// and its port")
parser.add_argument("local_token_file", type=Path, help="The machine-local token file; its contents never enter argv")
parser.add_argument("--ca-file", type=Path, help="Trusted CA PEM for a private HTTPS gateway certificate")
args = parser.parse_args()
origin = urllib.parse.urlsplit(args.gateway_origin)
if (origin.scheme not in {"http", "https"}
        or origin.hostname not in {"localhost", "127.0.0.1", "::1"}
        or origin.username is not None or origin.password is not None
        or origin.path not in {"", "/"} or origin.query or origin.fragment):
    parser.error("gateway-origin must be a loopback http(s) origin without credentials, path, query or fragment")
try:
    token = args.local_token_file.read_text().strip()
    if not token or not token.isascii() or any(ord(c) < 33 or ord(c) > 126 for c in token):
        raise ValueError("local token file is empty or invalid")
    url = urllib.parse.urlunsplit((origin.scheme, origin.netloc, "/~/api/v1/control/workspaces", "", ""))
    request = urllib.request.Request(url, headers={"x-lucidos-local-token": token})
    context = ssl.create_default_context(cafile=args.ca_file)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(),
                                         urllib.request.HTTPSHandler(context=context))
    with opener.open(request, timeout=10) as response:
        payload = json.load(response)
    rows = payload["workspaces"]
    if not isinstance(rows, list):
        raise ValueError("gateway workspaces field is not a list")
    output = []
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("gateway workspace entry is not an object")
        name, health = row["name"], row["health"]
        if not isinstance(name, str) or not isinstance(health, str):
            raise ValueError("gateway workspace name and health must be strings")
        output.append(f"{json.dumps(name, ensure_ascii=False)}\t{json.dumps(health)}")
    for line in output:
        print(line)
except (OSError, ValueError, KeyError, TypeError, http.client.HTTPException, urllib.error.URLError) as error:
    print(f"Cannot read gateway workspace health: {error}", file=sys.stderr)
    sys.exit(1)
PY
