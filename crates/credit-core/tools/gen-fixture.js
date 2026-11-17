#!/usr/bin/env node
// 用 Node 侧（与原版一致的 crypto）生成 tc 信封固定向量，供 Rust 测试交叉验证。
// 用法：node gen-fixture.js > trae_envelope_fixture.json
const crypto = require('crypto');
const WOE = [82,9,106,213,48,54,165,56,191,64,163,158,129,243,215,251,124,227,57,130,155,47,255,135,52,142,67,68,196,222,233,203,84,123,148,50,166,194,35,61,238,76,149,11,66,250,195,78,8,46,161,102,40,217,36,178,118,91,162,73,109,139,209,37];
const VOE = [31,221,168,51,136,7,199,49,177,18,16,89,39,128,236,95,96,81,127,169,25,181,74,13,45,229,122,159,147,201,156,239,160,224,59,77,174,42,245,176,200,235,187,60,131,83,153,97,23,43,4,126,186,119,214,38,225,105,20,99,85,33,12,125];
const HEADER = Buffer.from([116,99,5,16,0,0]);
const sha512 = (b) => crypto.createHash('sha512').update(b).digest();
const pepper = Buffer.from(WOE.map((v, i) => v ^ VOE[i]));
const body = Buffer.from(JSON.stringify({
  token: 'eyJhbGciOiJFUzI1NiJ9.eyJkYXRhIjp7ImlkIjoiNDU2Nzg5In0sImV4cCI6OTk5OTk5OTk5OX0.sig',
  host: 'https://api.trae.cn',
  expireAt: 9999999999,
}));
const salt = crypto.randomBytes(32);
const k = sha512(Buffer.concat([sha512(salt), pepper]));
const cipher = crypto.createCipheriv('aes-128-cbc', k.subarray(0, 16), k.subarray(16, 32));
const plain = Buffer.concat([sha512(body), body]);
const ct = Buffer.concat([cipher.update(plain), cipher.final()]);
const raw = Buffer.concat([HEADER, salt, ct]);
console.log(JSON.stringify({ envelope: raw.toString('base64') }, null, 2));
