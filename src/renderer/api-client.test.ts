import assert from 'node:assert/strict';
import test from 'node:test';
import { readApiResult } from './api-client.ts';

test('工作台的 JSON 错误原样交给界面', () => {
  assert.throws(() => readApiResult(400, '{"ok":false,"error":"找不到这个会话"}'), /找不到这个会话/);
  assert.equal(readApiResult(200, '{"ok":true,"data":{"tid":"t1"}}').tid, 't1');
});

test('裸的 413 用上传上限说明，而不是 JSON 解析失败', () => {
  assert.throws(() => readApiResult(413, 'length limit exceeded'), /上传内容超过 64MB/);
  assert.throws(() => readApiResult(413, ''), /上传内容超过 64MB/);
  assert.throws(
    () => readApiResult(413, '{"ok":false,"error":"上传内容超过 64MB"}'),
    /上传内容超过 64MB/,
  );
});

test('其他非 JSON 失败保留状态码', () => {
  assert.throws(() => readApiResult(502, '<html>bad gateway</html>'), /bad gateway/);
  assert.throws(() => readApiResult(500, ''), /请求失败 \(500\)/);
});
