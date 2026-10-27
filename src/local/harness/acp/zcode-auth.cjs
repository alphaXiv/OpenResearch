const {readFileSync} = require('node:fs');
const {dirname, join} = require('node:path');
const {pathToFileURL} = require('node:url');
const oauthAccountForAuth = module.exports.oauthAccount;
async function check() {
  const dist = dirname(module.exports.adapterEntry(process.argv[1]));
  const utils = await import(pathToFileURL(join(dist, 'utils.js')).href);
  for (const path of [utils.ZCODE_CREDS_PATH, utils.zcodePersonalProviderPath()]) {
    try { JSON.parse(readFileSync(path, 'utf8')); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  const options = await import(pathToFileURL(join(dist, 'config/options.js')).href);
  const account = await import(pathToFileURL(join(dist, 'config/account-provider.js')).href);
  const credentials = await import(pathToFileURL(join(dist, 'backend/credential-store.js')).href);
  let selection;
  try { selection = JSON.parse(readFileSync(utils.zcodePersonalProviderPath(), 'utf8')).config?.defaultModelSelection; }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  const oauth = oauthAccountForAuth(credentials.readCredentialStoreSync(), selection);
  if (oauth) return oauth.expired ? 'expired' : 'configured';
  const selected = options.loadAllModels()[0];
  if (!selected) return 'missing';
  if (options.isBuiltinProvider(selected.providerId) || selected.providerId.startsWith('account:')) {
    const id = account.accountProviderIdFor(selected.providerId);
    const snapshot = account.buildAccountProviderConfig();
    const state = snapshot?.states?.[id];
    if (state?.availability === 'unavailable') return account.codingPlanRequestAuthFor(id) ? 'blocked' : 'missing';
    if (state?.availability !== 'available') return 'unknown';
    return account.codingPlanRequestAuthFor(id) ? 'configured' : 'missing';
  }
  const provider = options.findProviderConfig(selected.providerId);
  if (provider?.options?.apiKey || provider?.options?.apiKeyRequired === false) return 'configured';
  const {zcodePersonalProviderPath} = await import(pathToFileURL(join(dist, 'utils.js')).href);
  const personal = JSON.parse(readFileSync(zcodePersonalProviderPath(), 'utf8'));
  const rule = personal.config?.providerConfigRules?.providerRules?.find(rule => rule.providerId === selected.providerId && rule.enabled !== false);
  return rule?.config?.access?.apiKey ? 'configured' : 'missing';
}
// The adapter decrypts its own credentials; only a status leaves this process.
check().then(status => process.stdout.write(status)).catch(() => process.stdout.write('unknown'));
