const {readFileSync, realpathSync} = require('node:fs');
const {dirname, join} = require('node:path');
const {pathToFileURL} = require('node:url');

function oauthAccount(store, selection) {
  if (selection?.providerId !== 'account:zai-individual-coding-plan' || !selection.modelId) return null;
  const token = store['oauth:zai:access_token'];
  const identity = store[`account-provider:${selection.providerId}:identity`];
  const source = store[`account-provider:${selection.providerId}:auth-source`];
  if (!token || !identity || source !== 'oauth') return null;
  let expires;
  try { expires = JSON.parse(Buffer.from(token.split('.')[1], 'base64url')).exp; } catch {}
  return {providerId: selection.providerId, modelId: selection.modelId, token, identity,
    expired: typeof expires === 'number' && expires * 1000 <= Date.now()};
}

async function requestAuth(account, request = fetch) {
  if (account.expired) throw new Error('Z.ai sign-in has expired. Sign in again in OpenResearch.');
  const origin = 'https://api.z.ai';
  async function json(path, method, token, body) {
    const response = await request(origin + path, {
      method, redirect: 'error', signal: AbortSignal.timeout(15000),
      headers: {'Content-Type': 'application/json', ...(token ? {Authorization: `Bearer ${token}`} : {})},
      ...(body ? {body: JSON.stringify(body)} : {}),
    });
    let result;
    try { result = await response.json(); }
    catch { throw new Error('Z.ai returned an invalid authorization response.'); }
    if (!response.ok || ![0, 200, '0', '200'].includes(result.code)) {
      throw new Error(response.status === 401 ? 'Z.ai sign-in has expired. Sign in again in OpenResearch.' : 'Z.ai could not authorize ZCode model access. Check your Z.ai account and model plan.');
    }
    return result.data;
  }
  const login = await json('/api/auth/z/login', 'POST', null, {token: account.token});
  const businessToken = login?.access_token ?? login?.accessToken;
  if (!businessToken) throw new Error('Z.ai did not return model authorization. Sign in again.');
  const customer = await json('/api/biz/customer/getCustomerInfo', 'GET', businessToken);
  const organizations = customer?.organizations ?? [];
  const organization = organizations.find(item => item.organizationName?.includes('默认机构')) ?? organizations[0];
  const project = organization?.projects?.find(item => item.projectName?.includes('默认项目')) ?? organization?.projects?.[0];
  if (!organization?.organizationId || !project?.projectId) throw new Error('Z.ai has no model project available. Configure model access in your Z.ai account.');
  const keysPath = `/api/biz/v1/organization/${encodeURIComponent(organization.organizationId)}/projects/${encodeURIComponent(project.projectId)}/api_keys`;
  const keys = await json(keysPath, 'GET', businessToken);
  if (!Array.isArray(keys)) throw new Error('Z.ai returned an invalid model-project response.');
  const key = keys.find(item => item.name === 'zcode-api-key') ?? await json(keysPath, 'POST', businessToken, {name: 'zcode-api-key', usageScene: 1});
  if (!key?.apiKey) throw new Error('Z.ai did not return ZCode model credentials.');
  const access = await json(`${keysPath}/${encodeURIComponent(key.apiKey)}/access_tokens`, 'POST', businessToken, {clientType: 'zcode', clientVersion: '0.16.9'});
  const token = access?.accessToken ?? access?.access_token ?? access?.token;
  if (!token || access.enable === false) throw new Error('Z.ai model access is disabled. Check your Z.ai model plan.');
  return {apiKey: token, apiKeyId: key.apiKey};
}

function adapterEntry(executable) {
  if (!/\.(cmd|ps1)$/i.test(executable)) return realpathSync(executable);
  const root = join(dirname(executable), 'node_modules', 'zcode-acp-server');
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  const bin = typeof pkg.bin === 'string' ? pkg.bin : pkg.bin?.['zcode-acp-server'];
  if (!bin) throw new Error('ZCode ACP adapter has no JavaScript launcher. Reinstall the adapter.');
  return join(root, bin);
}

async function launch(executable) {
  const entry = adapterEntry(executable);
  const dist = dirname(entry);
  const pkg = JSON.parse(readFileSync(join(dist, '..', 'package.json'), 'utf8'));
  if (pkg.name !== 'zcode-acp-server' || !['0.69.2', '0.69.3'].includes(pkg.version)) {
    const child = require('node:child_process').spawn(process.execPath, [entry], {stdio: 'inherit'});
    child.on('exit', code => { process.exitCode = code ?? 1; });
    return;
  }
  const load = path => import(pathToFileURL(join(dist, path)).href);
  const credentials = await load('backend/credential-store.js');
  const utils = await load('utils.js');
  let selection;
  try { selection = JSON.parse(readFileSync(utils.zcodePersonalProviderPath(), 'utf8')).config?.defaultModelSelection; }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  const readAccount = () => oauthAccount(credentials.readCredentialStoreSync(), selection);
  const account = readAccount();
  if (account && !process.env.ZCODE_PROVIDER && !process.env.ZCODE_MODEL) {
    process.env.ZCODE_PROVIDER = account.providerId;
    process.env.ZCODE_MODEL = account.modelId;
  }
  if (account) {
    const {ZcodeAcpServer} = await load('server.js');
    const original = ZcodeAcpServer.prototype.ensureBackend;
    const wrapped = new WeakSet();
    // These adapter releases predate the CLI's standalone OAuth credential pair.
    ZcodeAcpServer.prototype.ensureBackend = async function () {
      const backend = await original.call(this);
      if (wrapped.has(backend)) return backend;
      wrapped.add(backend);
      const request = backend.request.bind(backend);
      backend.request = (id, method, params, ...rest) => {
        if (method === 'provider/updateAccountConfig') {
          const current = readAccount();
          if (current) {
            params = {...params, providers: {...params.providers,
              [current.providerId]: {...params.providers[current.providerId], access: {type: 'zhipu-account', entitled: !current.expired}}},
              states: {...params.states, [current.providerId]: {availability: current.expired ? 'unavailable' : 'available', entitled: !current.expired, current: true}}};
          }
        }
        return request(id, method, params, ...rest);
      };
      const respond = backend.providerRuntimeHeadersResponder;
      backend.providerRuntimeHeadersResponder = (id, params) => {
        const current = readAccount();
        const providerId = params.modelSelection?.providerId ?? params.providerId;
        if (providerId !== account.providerId) return respond?.(id, params) ?? false;
        if (!current) {
          backend.sendReply(id, {headersApplied: false, errorMessage: 'Z.ai sign-in is missing. Sign in again in OpenResearch.'});
          return true;
        }
        requestAuth(current).then(auth => {
          const confirmed = readAccount();
          if (confirmed?.token !== current.token || confirmed?.identity !== current.identity) throw new Error('Z.ai account changed during authorization. Send a new message after signing in.');
          backend.sendReply(id, {headersApplied: true, requestAuth: auth});
        }).catch(
          error => backend.sendReply(id, {headersApplied: false, errorMessage: error.message}));
        return true;
      };
      return backend;
    };
  }
  await (await load('index.js')).main();
}

module.exports = {oauthAccount, requestAuth, adapterEntry};
if (process.argv[2] === '--orx-launch') launch(process.argv[1]).catch(error => {
  process.stderr.write(`ZCode ACP startup failed: ${error.message}\n`);
  process.exitCode = 1;
});
