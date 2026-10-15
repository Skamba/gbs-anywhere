const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const html = fs.readFileSync(path.join(__dirname, '../main/index.html'), 'utf8');
const translations = html.match(/<script id="translations" type="application\/json">([\s\S]*?)<\/script>/)[1];
const messages = JSON.parse(translations);
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
const keys = Object.keys(messages.en).sort();
assert.deepEqual(Object.keys(messages), ['en', 'de', 'fr', 'it', 'ja', 'ko', 'zh-CN', 'zh-TW']);
for (const dictionary of Object.values(messages)) {
  assert.deepEqual(Object.keys(dictionary).sort(), keys);
  assert.ok(Object.values(dictionary).every(value => typeof value === 'string' && value.trim()));
}

async function page({languages = ['en-US'], saved = null, blocked = false} = {}) {
  const nodes = Object.fromEntries([...html.matchAll(/\bid="([^"]+)"/g)].map(match => [match[1], {
    textContent: '', value: '', hidden: false, events: {},
    addEventListener(name, callback) { this.events[name] = callback; }
  }]));
  nodes.translations.textContent = translations;
  const labels = [...html.matchAll(/data-i18n="([^"]+)"/g)].map(match => {
    assert.ok(keys.includes(match[1]));
    return {dataset: {i18n: match[1]}, textContent: ''};
  });
  const document = {documentElement: {}, getElementById: id => nodes[id], querySelectorAll: () => labels};
  const model = {mode: 'active', phase: 'waiting_for_coffee', grinder: true, cloud: false,
    last_outcome: 'success', latest_coffee_ms: 1, latest_seconds: 30.5, latest_grams: 36,
    target_seconds: 30, successes: 1, uptime_s: 600, free_internal: 102400, free_psram: 1048576};
  let stored = saved, online = true, calls = 0;
  const navigator = {languages};
  const context = vm.createContext({document, navigator, window: {addEventListener() {}},
    localStorage: {
      getItem() { if (blocked) throw Error('blocked'); return stored; },
      setItem(key, value) { if (blocked) throw Error('blocked'); stored = value; }
    },
    async fetch(url, options) {
      assert.equal(url, '/api/state');
      assert.equal(options.method, undefined);
      calls++;
      return {ok: online, status: 503, async json() { return model; }};
    },
    setTimeout() {}
  });
  vm.runInContext(script, context);
  const refresh = () => vm.runInContext('refresh()', context);
  await refresh();
  return {nodes, document, labels, model, navigator, refresh,
    saved: () => stored, calls: () => calls,
    online(value) { online = value; },
    choose(value) { nodes.language.value = value; nodes.language.events.change(); }};
}

(async () => {
  for (const [languages, expected] of [
    [['fr-CA'], 'fr'], [['de-DE'], 'de'], [['it-CH'], 'it'], [['ja-JP'], 'ja'], [['ko-KR'], 'ko'],
    [['zh-Hans'], 'zh-CN'], [['zh-HK'], 'zh-TW'], [['zh-Hant-CN'], 'zh-TW'],
    [['zh-Hans-TW'], 'zh-CN'], [['es-ES', 'de-DE'], 'de'], [['es-ES'], 'en']
  ]) assert.equal((await page({languages})).document.documentElement.lang, expected);

  const p = await page({languages: ['zh-CN']});
  for (const locale of Object.keys(messages)) {
    const calls = p.calls();
    p.choose(locale);
    assert.equal(p.calls(), calls, 'Language changes must not send device requests');
    assert.equal(p.document.documentElement.lang, locale);
    assert.equal(p.saved(), locale);
    assert.equal(p.nodes.phase.textContent, messages[locale].waiting_for_coffee);
    assert.equal(p.nodes.outcome.textContent, messages[locale].success);
    assert.equal(p.nodes.cloud.textContent, messages[locale].disconnected);
    assert.ok(p.labels.every(label => label.textContent === messages[locale][label.dataset.i18n]));
  }
  p.choose('fr');
  assert.ok(p.nodes.coffee.textContent.includes('30,5'));
  assert.equal((await page({saved: p.saved(), languages: ['ko-KR']})).document.documentElement.lang, 'fr');
  assert.equal((await page({saved: 'invalid', languages: ['ja']})).document.documentElement.lang, 'ja');
  const denied = await page({blocked: true, languages: ['de']});
  denied.choose('ko');
  assert.equal(denied.document.documentElement.lang, 'ko');
  p.choose('auto');
  assert.equal(p.document.documentElement.lang, 'zh-CN');
  p.model.last_outcome = 'new_diagnostic_code';
  p.model.issue = 'brewDone: ClientErrorOngoingJobNotExisting';
  await p.refresh();
  assert.equal(p.nodes.outcome.textContent, 'new_diagnostic_code');
  assert.equal(p.nodes.issue.textContent, p.model.issue);
  assert.equal(p.nodes.diagnostic.hidden, false);
  p.online(false);
  await p.refresh();
  assert.equal(p.nodes.summary.textContent, messages['zh-CN'].unreachable);
  p.online(true);
  Object.keys(p.model).forEach(key => delete p.model[key]);
  p.model.phase = 'booting';
  await p.refresh();
  assert.equal(p.nodes.phase.textContent, messages['zh-CN'].booting);
  assert.equal(p.nodes.uptime.textContent, '—');
  assert.equal(p.nodes.summary.textContent, messages['zh-CN'].reading);
  console.log('PASS: 8 locales, negotiation, persistence, denied storage, localized units, boot and offline states');
})().catch(error => { console.error(error); process.exitCode = 1; });
