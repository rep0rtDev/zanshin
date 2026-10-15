/** XXH3-64 (seed 0, default secret) in pure TypeScript — matches the server's bucketing. */

const M64 = (1n << 64n) - 1n;
const M32 = 0xffffffffn;
const P32_1 = 0x9e3779b1n, P32_2 = 0x85ebca77n, P32_3 = 0xc2b2ae3dn;
const P64_1 = 0x9e3779b185ebca87n, P64_2 = 0xc2b2ae3d27d4eb4fn, P64_3 = 0x165667b19e3779f9n, P64_4 = 0x85ebca77c2b2ae63n, P64_5 = 0x27d4eb2f165667c5n;
const MX1 = 0x165667919e3779f9n, MX2 = 0x9fb21c651e98df25n;
const SECRET = Uint8Array.from([
  0xb8, 0xfe, 0x6c, 0x39, 0x23, 0xa4, 0x4b, 0xbe, 0x7c, 0x01, 0x81, 0x2c, 0xf7, 0x21, 0xad, 0x1c, 0xde, 0xd4, 0x6d, 0xe9, 0x83, 0x90, 0x97, 0xdb, 0x72, 0x40, 0xa4, 0xa4, 0xb7, 0xb3, 0x67, 0x1f,
  0xcb, 0x79, 0xe6, 0x4e, 0xcc, 0xc0, 0xe5, 0x78, 0x82, 0x5a, 0xd0, 0x7d, 0xcc, 0xff, 0x72, 0x21, 0xb8, 0x08, 0x46, 0x74, 0xf7, 0x43, 0x24, 0x8e, 0xe0, 0x35, 0x90, 0xe6, 0x81, 0x3a, 0x26, 0x4c,
  0x3c, 0x28, 0x52, 0xbb, 0x91, 0xc3, 0x00, 0xcb, 0x88, 0xd0, 0x65, 0x8b, 0x1b, 0x53, 0x2e, 0xa3, 0x71, 0x64, 0x48, 0x97, 0xa2, 0x0d, 0xf9, 0x4e, 0x38, 0x19, 0xef, 0x46, 0xa9, 0xde, 0xac, 0xd8,
  0xa8, 0xfa, 0x76, 0x3f, 0xe3, 0x9c, 0x34, 0x3f, 0xf9, 0xdc, 0xbb, 0xc7, 0xc7, 0x0b, 0x4f, 0x1d, 0x8a, 0x51, 0xe0, 0x4b, 0xcd, 0xb4, 0x59, 0x31, 0xc8, 0x9f, 0x7e, 0xc9, 0xd9, 0x78, 0x73, 0x64,
  0xea, 0xc5, 0xac, 0x83, 0x34, 0xd3, 0xeb, 0xc3, 0xc5, 0x81, 0xa0, 0xff, 0xfa, 0x13, 0x63, 0xeb, 0x17, 0x0d, 0xdd, 0x51, 0xb7, 0xf0, 0xda, 0x49, 0xd3, 0x16, 0x55, 0x26, 0x29, 0xd4, 0x68, 0x9e,
  0x2b, 0x16, 0xbe, 0x58, 0x7d, 0x47, 0xa1, 0xfc, 0x8f, 0xf8, 0xb8, 0xd1, 0x7a, 0xd0, 0x31, 0xce, 0x45, 0xcb, 0x3a, 0x8f, 0x95, 0x16, 0x04, 0x28, 0xaf, 0xd7, 0xfb, 0xca, 0xbb, 0x4b, 0x40, 0x7e,
]);

const r64 = (b: Uint8Array, i: number): bigint => {
  let v = 0n;
  for (let k = 7; k >= 0; k--) v = (v << 8n) | BigInt(b[i + k]!);
  return v;
};
const r32 = (b: Uint8Array, i: number): bigint => BigInt(b[i]! | (b[i + 1]! << 8) | (b[i + 2]! << 16)) | (BigInt(b[i + 3]!) << 24n);
const mul = (a: bigint, b: bigint): bigint => (a * b) & M64;
const rotl = (x: bigint, r: bigint): bigint => ((x << r) | (x >> (64n - r))) & M64;
const swap64 = (x: bigint): bigint => {
  let o = 0n;
  for (let i = 0; i < 8; i++) o = (o << 8n) | ((x >> BigInt(i * 8)) & 0xffn);
  return o;
};
const fold = (a: bigint, b: bigint): bigint => {
  const p = a * b;
  return ((p & M64) ^ (p >> 64n)) & M64;
};
const avalanche = (h: bigint): bigint => {
  h ^= h >> 37n;
  h = mul(h, MX1);
  return h ^ (h >> 32n);
};
const avalanche64 = (h: bigint): bigint => {
  h ^= h >> 33n;
  h = mul(h, P64_2);
  h ^= h >> 29n;
  h = mul(h, P64_3);
  return h ^ (h >> 32n);
};
const mix16 = (b: Uint8Array, i: number, s: number): bigint =>
  fold(r64(b, i) ^ r64(SECRET, s), r64(b, i + 8) ^ r64(SECRET, s + 8));

function long(b: Uint8Array): bigint {
  const len = b.length;
  const acc = [P32_3, P64_1, P64_2, P64_3, P64_4, P32_2, P64_5, P32_1];
  const acc512 = (i: number, s: number) => {
    for (let k = 0; k < 8; k++) {
      const dv = r64(b, i + 8 * k);
      const dk = dv ^ r64(SECRET, s + 8 * k);
      acc[k ^ 1] = (acc[k ^ 1]! + dv) & M64;
      acc[k] = (acc[k]! + (dk & M32) * (dk >> 32n)) & M64;
    }
  };
  const scramble = () => {
    for (let k = 0; k < 8; k++) {
      let a = acc[k]!;
      a ^= a >> 47n;
      a ^= r64(SECRET, 128 + 8 * k);
      acc[k] = mul(a, P32_1);
    }
  };
  const blocks = Math.floor((len - 1) / 1024);
  for (let n = 0; n < blocks; n++) {
    for (let s = 0; s < 16; s++) acc512(n * 1024 + s * 64, s * 8);
    scramble();
  }
  const stripes = Math.floor((len - 1 - 1024 * blocks) / 64);
  for (let s = 0; s < stripes; s++) acc512(blocks * 1024 + s * 64, s * 8);
  acc512(len - 64, 192 - 64 - 7);
  let r = mul(BigInt(len), P64_1);
  for (let k = 0; k < 4; k++) r = (r + fold(acc[2 * k]! ^ r64(SECRET, 11 + 16 * k), acc[2 * k + 1]! ^ r64(SECRET, 19 + 16 * k))) & M64;
  return avalanche(r);
}

/** 64-bit XXH3 of `input` (string → UTF-8, or raw bytes). */
export function xxh3_64(input: string | Uint8Array): bigint {
  const b = typeof input === 'string' ? new TextEncoder().encode(input) : input;
  const len = b.length;
  if (len === 0) return avalanche64(r64(SECRET, 56) ^ r64(SECRET, 64));
  if (len <= 3) {
    const c1 = BigInt(b[0]!), c2 = BigInt(b[len >> 1]!), c3 = BigInt(b[len - 1]!);
    const combined = ((c1 << 16n) | (c2 << 24n) | c3 | (BigInt(len) << 8n)) & M32;
    return avalanche64(combined ^ ((r32(SECRET, 0) ^ r32(SECRET, 4)) & M32));
  }
  if (len <= 8) {
    const in1 = r32(b, 0), in2 = r32(b, len - 4);
    const bitflip = r64(SECRET, 8) ^ r64(SECRET, 16);
    let h = ((in2 + (in1 << 32n)) ^ bitflip) & M64;
    h ^= rotl(h, 49n) ^ rotl(h, 24n);
    h = mul(h, MX2);
    h ^= (h >> 35n) + BigInt(len);
    h = mul(h, MX2);
    return h ^ (h >> 28n);
  }
  if (len <= 16) {
    const lo = r64(b, 0) ^ (r64(SECRET, 24) ^ r64(SECRET, 32));
    const hi = r64(b, len - 8) ^ (r64(SECRET, 40) ^ r64(SECRET, 48));
    const acc = (BigInt(len) + swap64(lo) + hi + fold(lo, hi)) & M64;
    return avalanche(acc);
  }
  if (len <= 128) {
    let acc = mul(BigInt(len), P64_1);
    if (len > 32) {
      if (len > 64) {
        if (len > 96) acc = (acc + mix16(b, 48, 96) + mix16(b, len - 64, 112)) & M64;
        acc = (acc + mix16(b, 32, 64) + mix16(b, len - 48, 80)) & M64;
      }
      acc = (acc + mix16(b, 16, 32) + mix16(b, len - 32, 48)) & M64;
    }
    acc = (acc + mix16(b, 0, 0) + mix16(b, len - 16, 16)) & M64;
    return avalanche(acc);
  }
  if (len <= 240) {
    let acc = mul(BigInt(len), P64_1);
    for (let i = 0; i < 8; i++) acc = (acc + mix16(b, 16 * i, 16 * i)) & M64;
    acc = avalanche(acc);
    const rounds = Math.floor(len / 16);
    for (let i = 8; i < rounds; i++) acc = (acc + mix16(b, 16 * i, 16 * (i - 8) + 3)) & M64;
    acc = (acc + mix16(b, len - 16, 119)) & M64;
    return avalanche(acc);
  }
  return long(b);
}

export const BUCKETS = 10000;

/** Server-compatible bucket: xxh3_64(namespace \0 salt \0 subject) % 10000. */
export function bucket(namespace: string, salt: string, subject: string): number {
  const enc = new TextEncoder();
  const a = enc.encode(namespace), s = enc.encode(salt), u = enc.encode(subject);
  const buf = new Uint8Array(a.length + s.length + u.length + 2);
  buf.set(a, 0);
  buf.set(s, a.length + 1);
  buf.set(u, a.length + s.length + 2);
  return Number(xxh3_64(buf) % BigInt(BUCKETS));
}

export function inRollout(namespace: string, salt: string, subject: string, percentage: number): boolean {
  if (percentage >= 100) return true;
  if (percentage <= 0) return false;
  return bucket(namespace, salt, subject) < Math.round((percentage / 100) * BUCKETS);
}

/** Index picked for `subject` by weight, or -1 when no weight is positive. */
export function weightedPick(namespace: string, salt: string, subject: string, weights: number[]): number {
  const total = weights.filter((w) => w > 0).reduce((a, b) => a + b, 0);
  if (total <= 0) return -1;
  const point = (bucket(namespace, salt, subject) / BUCKETS) * total;
  let acc = 0;
  for (let i = 0; i < weights.length; i++) {
    const w = weights[i]!;
    if (w <= 0) continue;
    acc += w;
    if (point < acc) return i;
  }
  for (let i = weights.length - 1; i >= 0; i--) if (weights[i]! > 0) return i;
  return -1;
}
