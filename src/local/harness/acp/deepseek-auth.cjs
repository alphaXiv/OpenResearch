const { execFileSync } = require('node:child_process');
const { readFileSync, realpathSync, statSync } = require('node:fs');
const { createRequire } = require('node:module');
const { homedir } = require('node:os');
const { join } = require('node:path');
const { parseEnv } = require('node:util');
const { pathToFileURL } = require('node:url');

function readOptional(path) {
  try { return readFileSync(path, 'utf8'); }
  catch (error) { if (error.code === 'ENOENT') return undefined; throw error; }
}

async function check() {
  const [executable, argumentsJson] = process.argv.slice(1);
  const vendor = createRequire(realpathSync(executable));
  const { parse } = vendor('yaml');
  const profile = parse(execFileSync(executable, [...JSON.parse(argumentsJson), '--dump-config'], {
    encoding: 'utf8', timeout: 10000, stdio: ['ignore', 'pipe', 'ignore'],
  }), { logLevel: 'silent' });
  if (!Array.isArray(profile)) return 'unknown';
  const acp = profile.find(entry => entry.name === '@deepseek-ai/dsh-acp');
  const credentials = profile.find(entry => entry.name === '@deepseek-ai/dsh-credentials-local');
  if (acp?.config?.provider !== 'deepseek-official' || !credentials) return 'unknown';
  const config = credentials.config ?? {};
  const home = config.dshHome ?? process.env.DSH_HOME ?? join(homedir(), '.dsh');
  if (typeof home !== 'string' || config.path !== undefined && typeof config.path !== 'string') return 'unknown';
  const path = config.path ?? join(home, '.credentials.yaml');
  const stored = readOptional(path);
  let configured = false;
  if (stored !== undefined) {
    if (process.platform !== 'win32' && (statSync(path).mode & 0o077)) return 'unknown';
    const { parseCredentialsDocument, renderFlatLayoutMigration } = await import(pathToFileURL(vendor.resolve('@deepseek-ai/dsh-credentials-local')).href);
    const document = parseCredentialsDocument(renderFlatLayoutMigration(stored) ?? stored, path);
    configured = Boolean(document.refs.get('DEEPSEEK_API_KEY'));
  }
  for (const path of [join(process.cwd(), '.env'), join(home, '.env')]) {
    const text = readOptional(path);
    if (text !== undefined) configured ||= Boolean(parseEnv(text).DEEPSEEK_API_KEY);
  }
  configured ||= Boolean(process.env.DEEPSEEK_API_KEY);
  return configured ? 'configured' : 'missing';
}

// Never forward provider configuration, credentials, or vendor error text.
check().then(status => process.stdout.write(status)).catch(() => process.stdout.write('unknown'));
