import { test } from 'node:test';
import assert from 'node:assert/strict';
import runtime from '../../src/local/acp/zcode-runtime.cjs';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
const { oauthAccount, requestAuth } = runtime;
const providerId = 'account:zai-individual-coding-plan';
const selection = {providerId, modelId: 'GLM-5.3'};
const jwt = exp => `header.${Buffer.from(JSON.stringify({exp})).toString('base64url')}.signature`;
const store = token => ({'oauth:zai:access_token': token,
  [`account-provider:${providerId}:identity`]: 'test-account',
  [`account-provider:${providerId}:auth-source`]: 'oauth'});

test('uses the selected OAuth account without inferring another plan', () => {
  assert.equal(oauthAccount(store(jwt(1)), selection).expired, true);
  assert.equal(oauthAccount(store(jwt(Date.now() / 1000 + 60)), selection).expired, false);
  assert.equal(oauthAccount({}, selection), null);
  assert.equal(oauthAccount(store(jwt(1)), {...selection, providerId: 'account:bigmodel-individual-coding-plan'}), null);
});

test('exchanges native OAuth through Z.ai only, retaining exact project/key IDs', async () => {
  const calls = [];
  const responses = [{access_token: 'business'}, {organizations: [{organizationId: 'org/a', projects: [{projectId: 'p/a'}]}]},
    [{name: 'zcode-api-key', apiKey: 'key/a'}], {accessToken: 'model-access', enable: true}];
  const request = async (url, options) => {
    calls.push({url, options});
    assert.equal(new URL(url).origin, 'https://api.z.ai');
    assert.equal(options.redirect, 'error');
    return {ok: true, status: 200, json: async () => ({code: 200, data: responses.shift()})};
  };
  assert.deepEqual(await requestAuth({token: 'oauth', expired: false}, request), {apiKey: 'model-access', apiKeyId: 'key/a'});
  assert.equal(calls.length, 4);
  assert.equal(JSON.parse(calls[0].options.body).token, 'oauth');
  assert.equal(calls[3].url, 'https://api.z.ai/api/biz/v1/organization/org%2Fa/projects/p%2Fa/api_keys/key%2Fa/access_tokens');
});

test('expiry and provider rejection fail without leaking credentials', async () => {
  await assert.rejects(requestAuth({expired: true}), /expired/);
  await assert.rejects(requestAuth({token: 'private-secret'}, async () => ({ok: false, status: 401, json: async () => ({code: 401})})), /expired/);
  await assert.rejects(requestAuth({token: 'private-secret'}, async () => ({ok: false, status: 429, json: async () => ({code: 1113, message: 'private-secret'})})), /Check your Z.ai account/);
});

test('Windows npm launchers resolve to the package JavaScript entry and preserve fallback launches', () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-zcode-npm-'));
  try {
    const pkg = join(root, 'node_modules', 'zcode-acp-server');
    mkdirSync(join(pkg, 'dist'), {recursive: true});
    writeFileSync(join(pkg, 'package.json'), JSON.stringify({name:'zcode-acp-server', version:'9.0.0', bin:{'zcode-acp-server':'dist/cli.js'}}));
    const entry = join(pkg, 'dist', 'cli.js');
    writeFileSync(entry, "process.stdout.write('npm adapter ready')");
    const launcher = join(root, 'zcode-acp-server.cmd');
    writeFileSync(launcher, '@echo off');
    assert.equal(runtime.adapterEntry(launcher), entry);
    const script = readFileSync(new URL('../../src/local/acp/zcode-runtime.cjs', import.meta.url), 'utf8');
    assert.equal(execFileSync(process.execPath, ['-e', script, launcher, '--orx-launch'], {encoding:'utf8'}), 'npm adapter ready');
  } finally {
    rmSync(root, {recursive:true});
  }
});

test('launcher preserves the ACP adapter while repairing account snapshot and runtime headers', () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-zcode-oauth-'));
  const dist = join(root, 'dist');
  mkdirSync(join(dist, 'backend'), {recursive: true});
  try {
    writeFileSync(join(root, 'package.json'), JSON.stringify({name: 'zcode-acp-server', version: '0.69.2', type: 'module'}));
    writeFileSync(join(root, 'personal.json'), JSON.stringify({config: {defaultModelSelection: selection}}));
    writeFileSync(join(dist, 'cli.js'), '');
    writeFileSync(join(dist, 'utils.js'), `export const zcodePersonalProviderPath = () => ${JSON.stringify(join(root, 'personal.json'))};`);
    writeFileSync(join(dist, 'backend/credential-store.js'), `export const readCredentialStoreSync = () => (${JSON.stringify(store(jwt(Date.now() / 1000 + 60)))});`);
    writeFileSync(join(dist, 'server.js'), `
      export class ZcodeAcpServer {
        async ensureBackend() {
          this.backend ??= {
            request: async (id, method, params) => params,
            providerRuntimeHeadersResponder: () => false,
            sendReply: (id, value) => this.reply(value),
          };
          return this.backend;
        }
      }
    `);
    writeFileSync(join(dist, 'index.js'), `
      import assert from 'node:assert/strict';
      import {ZcodeAcpServer} from './server.js';
      export async function main() {
        const responses = [{access_token:'business'}, {organizations:[{organizationId:'org',projects:[{projectId:'project'}]}]}, [{name:'zcode-api-key',apiKey:'key'}], {accessToken:'model-access',enable:true}];
        globalThis.fetch = async () => ({ok:true,status:200,json:async () => ({code:200,data:responses.shift()})});
        const server = new ZcodeAcpServer();
        const backend = await server.ensureBackend();
        assert.equal(await server.ensureBackend(), backend);
        assert.equal(process.env.ZCODE_PROVIDER, '${providerId}');
        assert.equal(process.env.ZCODE_MODEL, 'GLM-5.3');
        const params = await backend.request(1, 'provider/updateAccountConfig', {providers:{'${providerId}':{access:{entitled:false}}},states:{}});
        assert.equal(params.providers['${providerId}'].access.entitled, true);
        const reply = new Promise(resolve => {server.reply = resolve;});
        assert.equal(backend.providerRuntimeHeadersResponder(2, {modelSelection:{providerId:'${providerId}'}}), true);
        assert.deepEqual(await reply, {headersApplied:true,requestAuth:{apiKey:'model-access',apiKeyId:'key'}});
        assert.equal(backend.providerRuntimeHeadersResponder(3, {providerId:'another-provider'}), false);
        process.stdout.write('compatibility verified');
      }
    `);
    const script = readFileSync(new URL('../../src/local/acp/zcode-runtime.cjs', import.meta.url), 'utf8');
    const env = {...process.env};
    delete env.ZCODE_PROVIDER;
    delete env.ZCODE_MODEL;
    assert.equal(execFileSync(process.execPath, ['-e', script, join(dist, 'cli.js'), '--orx-launch'], {env, encoding: 'utf8'}), 'compatibility verified');
  } finally {
    rmSync(root, {recursive: true});
  }
});
