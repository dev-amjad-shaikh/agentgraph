#!/usr/bin/env python3
"""agentgraph Studio dev server — same-origin static host + API proxy.

Why this exists: agentgraph-server v0.3+ sends permissive CORS headers, so
studio/index.html can be opened straight from disk and talk to the server
directly — this proxy is *optional*. It remains useful for older servers
without CORS headers and for setups where same-origin is simply more
convenient: it serves studio/index.html AND proxies /api/* to the real
server, making the Studio and the API same-origin — no CORS involved.

Usage:
    python3 studio/serve.py [--port 8000] [--target http://127.0.0.1:8100]

Then open http://127.0.0.1:8000/ and connect to base URL  /api  (the field
accepts relative URLs; the page defaults to the real server, so type /api).
"""

import argparse
import http.client
import http.server
import pathlib
import urllib.parse

ROOT = pathlib.Path(__file__).resolve().parent


class Handler(http.server.SimpleHTTPRequestHandler):
    target_host = "127.0.0.1"
    target_port = 8100

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    # -- static -----------------------------------------------------------
    def do_GET(self):
        path = urllib.parse.urlsplit(self.path).path
        if path == "/api" or path.startswith("/api/"):
            self._proxy()
        else:
            if path in ("/", ""):
                self.path = "/index.html"
            super().do_GET()

    # -- proxy ------------------------------------------------------------
    def do_POST(self):
        self._proxy()

    def do_PUT(self):
        self._proxy()

    def do_DELETE(self):
        self._proxy()

    def _proxy(self):
        upstream = self.path[len("/api"):] or "/"
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            self.send_error(400, "malformed Content-Length header")
            return
        body = self.rfile.read(length) if length else None

        conn = http.client.HTTPConnection(self.target_host, self.target_port, timeout=600)
        headers = {}
        for name in ("Content-Type", "X-Api-Key", "Accept", "Last-Event-ID"):
            if self.headers.get(name):
                headers[name] = self.headers[name]
        try:
            conn.request(self.command, upstream, body=body, headers=headers)
            resp = conn.getresponse()
        except OSError as exc:
            self.send_error(502, f"proxy cannot reach {self.target_host}:{self.target_port} — {exc}")
            return

        self.send_response(resp.status)
        # Forward content type; never forward content-length — we stream.
        ct = resp.getheader("Content-Type")
        if ct:
            self.send_header("Content-Type", ct)
        # SSE: disable any buffering so events flush per frame.
        if ct and "text/event-stream" in ct:
            self.send_header("Cache-Control", "no-cache")
            self.send_header("X-Accel-Buffering", "no")
        self.end_headers()

        try:
            while True:
                chunk = resp.read1(4096) if hasattr(resp, "read1") else resp.read(4096)
                if not chunk:
                    break
                self.wfile.write(chunk)
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            conn.close()

    def log_message(self, fmt, *args):  # quieter logs
        pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--target", default="http://127.0.0.1:8100")
    args = ap.parse_args()

    parsed = urllib.parse.urlparse(args.target)
    Handler.target_host = parsed.hostname or "127.0.0.1"
    Handler.target_port = parsed.port or 80

    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"agentgraph Studio  →  http://127.0.0.1:{args.port}/")
    print(f"proxying /api/*    →  {args.target}")
    print(f"connect with base URL:  /api")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
