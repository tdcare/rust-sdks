/**
 * LiveKit Token 生成脚本 (Node.js)
 *
 * 用法:
 *   node gen_token.js
 *
 * 或者修改下方配置后生成自定义 token:
 *   node gen_token.js <apiKey> <apiSecret> <identity> <room>
 */

const crypto = require('crypto');

// ============ 配置 ============
const API_KEY = 'APIa7rwJhiQvEn4';
const API_SECRET = 'oUr51grszYtwicAZIIhwvPCDkiZxHmZNk7lcG1KNc1E';
const IDENTITY = 'web_initiator_mq12i0h3';
const ROOM = 'rrt_call_7467227352167096291';
const TTL_HOURS = 6; // token 有效期(小时)
// =============================

/**
 * Base64 URL-safe 编码 (不带填充)
 */
function base64UrlEncode(data) {
  return Buffer.from(data)
    .toString('base64')
    .replace(/=/g, '')
    .replace(/\+/g, '-')
    .replace(/\//g, '_');
}

/**
 * 使用 HMAC-SHA256 签名
 */
function hmacSha256(key, data) {
  return crypto.createHmac('sha256', key).update(data).digest();
}

/**
 * 生成 JWT token
 */
function generateToken(apiKey, apiSecret, identity, room, ttlHours) {
  const now = Math.floor(Date.now() / 1000);
  const exp = now + ttlHours * 3600;

  // JWT Header
  const header = {
    typ: 'JWT',
    alg: 'HS256',
  };

  // Claims (LiveKit 格式)
  const claims = {
    exp: exp,
    iss: apiKey,
    nbf: now,
    sub: identity,
    name: '',
    video: {
      roomCreate: false,
      roomList: false,
      roomRecord: false,
      roomAdmin: false,
      roomJoin: true,
      room: room,
      destinationRoom: '',
      canPublish: true,
      canSubscribe: true,
      canPublishData: true,
      canPublishSources: [],
      canUpdateOwnMetadata: false,
      ingressAdmin: false,
      hidden: false,
      recorder: false,
    },
    sip: {
      admin: false,
      call: false,
    },
    sha256: '',
    metadata: '',
    attributes: {},
    roomConfig: null,
  };

  // 编码 Header + Claims
  const headerB64 = base64UrlEncode(JSON.stringify(header));
  const claimsB64 = base64UrlEncode(JSON.stringify(claims));
  const signingInput = `${headerB64}.${claimsB64}`;

  // HMAC-SHA256 签名
  const signature = base64UrlEncode(hmacSha256(apiSecret, signingInput));

  return `${signingInput}.${signature}`;
}

// ============ 主流程 ============

// 支持命令行参数覆盖
const apiKey = process.argv[2] || API_KEY;
const apiSecret = process.argv[3] || API_SECRET;
const identity = process.argv[4] || IDENTITY;
const room = process.argv[5] || ROOM;

const token = generateToken(apiKey, apiSecret, identity, room, TTL_HOURS);

console.log('Token:');
console.log(token);
console.log();
console.log('Claims (decoded):');
const parts = token.split('.');
const claimsJson = Buffer.from(parts[1], 'base64').toString('utf-8');
console.log(JSON.stringify(JSON.parse(claimsJson), null, 2));
