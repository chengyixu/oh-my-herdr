// Resolve only native Pi settings. No session, model call, or credential mutation.
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
const deadline = setTimeout(() => process.exit(124), 10_000);
try {
  const candidate = (process.env.PATH || '').split(path.delimiter)
    .map(directory => path.resolve(directory, 'pi'))
    .find(file => { try { fs.accessSync(file, fs.constants.X_OK); return fs.statSync(file).isFile(); } catch { return false; } });
  if (!candidate) throw new Error('native Pi executable unavailable');
  const cli = fs.realpathSync(candidate);
  const sdk = await import(pathToFileURL(path.join(path.dirname(cli), 'index.js')).href);
  if (typeof sdk.SessionManager?.prototype?.appendContextEdit !== 'function') throw new Error('native context edits unavailable');
  const settings = sdk.SettingsManager.create(process.argv[1]);
  const provider = settings.getDefaultProvider();
  const model = settings.getDefaultModel();
  if (typeof provider !== 'string' || !provider || typeof model !== 'string' || !model) {
    throw new Error('Set a native Pi default provider/model before importing a foreign session');
  }
  process.stdout.write(JSON.stringify({ provider, model, block_images: settings.getBlockImages() }) + '\n');
} catch (error) {
  process.stderr.write('Native Pi settings discovery failed: ' + (error?.message || 'unavailable') + '\n');
  process.exitCode = 1;
} finally { clearTimeout(deadline); }
