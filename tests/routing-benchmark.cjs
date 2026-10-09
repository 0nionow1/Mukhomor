// Fresh HTTP connections measure route setup; all origins are loopback fixtures.
const http = require('node:http');
const fs = require('node:fs');
const assert = require('node:assert/strict');
const [proxy, origin, iterations, output] = process.argv.slice(2);
const samples = [];
async function request(host) {
  const start = process.hrtime.bigint();
  await new Promise((resolve, reject) => {
    const req = http.request({hostname: '127.0.0.1', port: Number(proxy),
      path: `http://${host}:${origin}/`, headers: {host: `${host}:${origin}`},
      agent: false, timeout: 3000}, res => {
      let body = '';
      res.setEncoding('utf8'); res.on('data', chunk => body += chunk);
      res.on('error', reject);
      res.on('end', () => {
        try {assert.equal(res.statusCode, 200); assert.equal(body, 'DIRECT_OK'); resolve();}
        catch (e) {reject(e);}
      });
    });
    req.on('error', reject); req.on('timeout', () => req.destroy(new Error('Benchmark timeout')));
    req.end();
  });
  return Number(process.hrtime.bigint() - start) / 1e6;
}
async function main() {
  const hosts = ['domain-hit.benchmark.test', 'ip-hit.benchmark.test'];
  for (let i = 0; i < 60; i++) await request(hosts[i % hosts.length]);
  const start = process.hrtime.bigint();
  for (let i = 0; i < Number(iterations); i++) samples.push(await request(hosts[i % hosts.length]));
  const elapsed = Number(process.hrtime.bigint() - start) / 1e6;
  samples.sort((a, b) => a - b);
  const percentile = p => samples[Math.min(samples.length - 1, Math.floor(samples.length * p))];
  fs.writeFileSync(output, JSON.stringify({connections: samples.length, elapsed_ms: elapsed,
    p50_ms: percentile(0.5), p95_ms: percentile(0.95), p99_ms: percentile(0.99),
    connections_per_second: samples.length * 1000 / elapsed}));
}
main().catch(e => {console.error(e.message); process.exitCode = 1;});
