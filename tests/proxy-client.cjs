const net = require('node:net');
const [proxyPort, originPort, host] = process.argv.slice(2);
const socket = net.connect(Number(proxyPort), '127.0.0.1');
let result = '';
socket.setTimeout(5000, () => { socket.destroy(); process.exitCode = 1; });
socket.on('connect', () => socket.write(`GET http://${host}:${originPort}/ HTTP/1.1\r\nHost: ${host}:${originPort}\r\nConnection: close\r\n\r\n`));
socket.on('data', data => result += data);
socket.on('end', () => { process.stdout.write(result); if (!result.includes('DIRECT_OK')) process.exitCode = 1; });
socket.on('error', () => { process.exitCode = 1; });
