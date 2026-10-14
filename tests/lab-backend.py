# Fake OpenAI backend: streams "Hel","lo" or returns one JSON body.
import http.server, json
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        self.send_response(200)
        if body.get("stream"):
            self.send_header("content-type", "text/event-stream"); self.end_headers()
            for t in ["Hel", "lo"]:
                self.wfile.write(f'data: {json.dumps({"choices":[{"delta":{"content":t}}]})}\n\n'.encode())
            self.wfile.write(b"data: [DONE]\n\n")
        else:
            self.send_header("content-type", "application/json"); self.end_headers()
            self.wfile.write(json.dumps({"choices":[{"message":{"content":"Hello from bob"}}]}).encode())
http.server.HTTPServer(("127.0.0.1", 8000), H).serve_forever()
