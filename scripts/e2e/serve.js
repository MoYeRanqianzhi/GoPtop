/** 静态 dist 服务器（SPA 回退 index.html）：node serve.js <port> <host> <distDir> */
const http = require("http");
const fs = require("fs");
const path = require("path");

const [, , portArg, hostArg, distArg] = process.argv;
const port = Number(portArg || 5173);
const host = hostArg || "127.0.0.1";
const dist = distArg || ".";
const MIME = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json" };

http.createServer((req, res) => {
  const urlPath = decodeURIComponent((req.url || "/").split("?")[0]);
  let file = path.join(dist, urlPath);
  if (!path.resolve(file).startsWith(path.resolve(dist))) { res.writeHead(403); res.end(); return; }
  if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) file = path.join(dist, "index.html");
  const ext = path.extname(file).toLowerCase();
  res.writeHead(200, { "Content-Type": MIME[ext] || "application/octet-stream" });
  fs.createReadStream(file).pipe(res);
}).listen(port, host, () => console.log(`serving ${dist} on http://${host}:${port}`));
