// Loopback-only HTTP origin used by the routing integration test.
const http = require('node:http');
const fs = require('node:fs');
const server = http.createServer((req, res) => {
  res.writeHead(200, { 'content-type': 'text/plain', connection: 'close' });
  res.end('DIRECT_OK');
});
server.listen(0, '127.0.0.1', () => fs.writeFileSync(process.argv[2], String(server.address().port)));
