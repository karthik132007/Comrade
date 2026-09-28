// Run from the repo root with Node, or JavaScriptCore's jsc executable.
// Exercises the real app event handlers with a small DOM/IPC substitute.
(function () {
  'use strict';
  var source = typeof require === 'function'
    ? require('node:fs').readFileSync('frontend/app.js', 'utf8')
    : readFile('frontend/app.js');
  var report = typeof print === 'function' ? print : console.log;
  function assert(condition, message) { if (!condition) throw new Error(message); }
  function deferred() {
    var result = {};
    result.promise = new Promise(function (resolve, reject) { result.resolve = resolve; result.reject = reject; });
    return result;
  }
  async function flush() { for (var i = 0; i < 30; i++) await Promise.resolve(); }

  function Element(tag) {
    this.tagName = tag;
    this.children = [];
    this.listeners = {};
    this.dataset = {};
    this.style = {};
    this.classList = { add: function () {}, remove: function () {}, toggle: function () {} };
    this.textContent = '';
    this.selectedIndex = -1;
    this._value = '';
  }
  Object.defineProperties(Element.prototype, {
    options: { get: function () { return this.children; } },
    innerHTML: { set: function () { this.children = []; this.selectedIndex = -1; } },
    value: {
      get: function () { return this.tagName === 'select' ? (this.children[this.selectedIndex] || {}).value || '' : this._value; },
      set: function (value) {
        if (this.tagName === 'select') this.selectedIndex = this.children.findIndex(function (c) { return c.value === value; });
        else this._value = String(value);
      },
    },
  });
  Element.prototype.appendChild = function (child) {
    this.children.push(child);
    if (this.tagName === 'select' && (this.selectedIndex < 0 || child.selected)) this.selectedIndex = this.children.length - 1;
  };
  Element.prototype.addEventListener = function (event, listener) {
    (this.listeners[event] || (this.listeners[event] = [])).push(listener);
  };
  Element.prototype.dispatch = function (event) {
    (this.listeners[event] || []).forEach(function (listener) { listener({ preventDefault: function () {} }); });
  };
  Element.prototype.querySelectorAll = function (selector) {
    var descendants = [];
    function visit(el) { el.children.forEach(function (child) { descendants.push(child); visit(child); }); }
    visit(this);
    return descendants.filter(function (el) {
      if (el.tagName !== 'input') return false;
      if (selector.indexOf(':checked') >= 0 && !el.checked) return false;
      var type = selector.match(/type="([^"]+)"/);
      var name = selector.match(/name="([^"]+)"/);
      return (!type || el.type === type[1]) && (!name || el.name === name[1]);
    });
  };
  Element.prototype.querySelector = function (selector) { return this.querySelectorAll(selector)[0] || null; };
  Element.prototype.scrollIntoView = Element.prototype.focus = function () {};

  var brave = '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser';
  var chrome = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
  var edge = '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
  async function app(savedExe) {
    var elements = {};
    var selects = ['pref-browser', 'pref-profile', 'voice-mic', 'ob-default', 'code-default', 'mem-kind'];
    var document = {
      body: new Element('body'),
      getElementById: function (id) { return elements[id] || (elements[id] = new Element(selects.indexOf(id) >= 0 ? 'select' : 'div')); },
      createElement: function (tag) { return new Element(tag); },
      querySelectorAll: function () { return []; },
      querySelector: function () { return null; },
    };
    ['user', 'comrade'].forEach(function (value) {
      var option = new Element('option'); option.value = value;
      document.getElementById('pref-profile').appendChild(option);
    });
    document.getElementById('settings').hidden = true;
    var state = {
      elements: elements,
      calls: [],
      handlers: {},
      saved: { browser: { exe: savedExe || brave, kind: 'binary', profile: 'user', headless: false, debug_port: 9222 }, voice: {}, coding: { agents: [], default: '' } },
    };
    var browsers = [
      { name: 'Brave', exe: brave, kind: 'binary', version: 'Brave 154.0', automation_supported: true },
      { name: 'Chrome', exe: chrome, kind: 'binary', version: 'Google Chrome 154.0.8037.57', automation_supported: true },
      { name: 'Edge', exe: edge, kind: 'binary', version: 'Microsoft Edge 154.0', automation_supported: true },
    ];
    function copy(value) { return JSON.parse(JSON.stringify(value)); }
    globalThis.document = document;
    globalThis.requestAnimationFrame = function () {};
    globalThis.window = {
      localStorage: { getItem: function () { return null; }, setItem: function () {}, removeItem: function () {} },
      __TAURI__: {
        event: { listen: function () {} },
        core: { invoke: function (command, args) {
          state.calls.push({ command: command, args: copy(args || {}) });
          if (state.handlers[command]) return state.handlers[command](args);
          if (command === 'app_info') return Promise.resolve({ onboarded: true, memory_kinds: [] });
          if (command === 'get_prefs') return Promise.resolve(copy(state.saved));
          if (command === 'system_browsers') return Promise.resolve(browsers);
          if (command === 'get_audio_devices') return Promise.resolve({ inputs: [] });
          if (command === 'voice_models_status') return Promise.resolve({ ready: true });
          if (command === 'save_browser_prefs') { state.saved.browser = copy(args.browser); return Promise.resolve(copy(state.saved)); }
          if (command === 'send_message') return Promise.resolve({ status: 'done' });
          if (command === 'start_voice_input') return Promise.resolve('started');
          return Promise.resolve([]);
        } },
      },
    };
    (0, eval)(source);
    await flush();
    elements['settings-btn'].dispatch('click');
    await flush();
    state.choose = function (exe) { elements['pref-browser'].value = exe; elements['pref-browser'].dispatch('change'); };
    state.send = function () { elements.input.value = 'open youtube'; elements.composer.dispatch('submit'); };
    state.count = function (command) { return state.calls.filter(function (c) { return c.command === command; }).length; };
    return state;
  }

  async function run() {
    var s = await app();
    var save = deferred();
    s.handlers.save_browser_prefs = function (args) { return save.promise.then(function () { s.saved.browser = args.browser; return s.saved; }); };
    s.choose(chrome);
    s.send();
    await flush();
    assert(s.count('save_browser_prefs') === 1, 'Browser change must save without coding agents or global Save');
    assert(s.count('save_prefs') === 0, 'Browser change must not write unrelated controls');
    assert(s.count('send_message') === 0, 'Message must wait for the browser save');
    assert(s.elements['browser-help'].hidden === false, 'Chrome 154 default profile must explain the limitation');
    save.resolve(); await flush();
    assert(s.saved.browser.exe === chrome && s.count('send_message') === 1, 'Message should use the saved Chrome selection');

    s = await app();
    var first = deferred(); var second = deferred();
    var writes = 0;
    s.handlers.save_browser_prefs = function () { writes++; return writes === 1 ? first.promise : second.promise; };
    s.choose(chrome); s.choose(edge); s.send(); await flush();
    assert(writes === 1, 'Rapid changes must serialize writes');
    first.resolve(); await flush();
    assert(writes === 2 && s.count('send_message') === 0, 'Task must wait for the newest selection');
    second.resolve(); await flush();
    assert(s.count('send_message') === 1 && s.elements['pref-browser'].value === edge, 'Latest selection must win');

    s = await app();
    var stale = deferred();
    var oldPrefs = JSON.parse(JSON.stringify(s.saved));
    s.handlers.get_prefs = function () { return stale.promise; };
    s.elements['settings-btn'].dispatch('click'); s.elements['settings-btn'].dispatch('click');
    await flush();
    s.choose(chrome); await flush();
    stale.resolve(oldPrefs); await flush();
    assert(s.elements['pref-browser'].value === chrome, 'Late settings load must not revert a newer browser choice');

    s = await app();
    s.handlers.save_browser_prefs = function () { return Promise.reject(new Error('Disk is full')); };
    s.choose(chrome); s.send(); await flush();
    assert(s.count('send_message') === 0 && s.elements.input.value === 'open youtube', 'Save failure must prevent wrong-browser execution and preserve the message');
    assert(s.elements['pref-status'].textContent.indexOf('Disk is full') >= 0, 'Save failure must be visible');
    delete s.handlers.save_browser_prefs;
    s.choose(chrome); s.send(); await flush();
    assert(s.count('send_message') === 1, 'A successful retry should unblock task start');

    s = await app('/Applications/Removed Browser.app/browser');
    assert(s.elements['pref-browser'].value === '/Applications/Removed Browser.app/browser', 'An unavailable saved browser must not display a different browser');

    s = await app();
    save = deferred();
    s.handlers.save_browser_prefs = function () { return save.promise; };
    s.choose(chrome); s.elements['mic-btn'].dispatch('mousedown'); await flush();
    assert(s.count('start_voice_input') === 0, 'Voice task must wait for browser save');
    s.elements['mic-btn'].dispatch('mouseup'); save.resolve(); await flush();
    assert(s.count('start_voice_input') === 0, 'Releasing the microphone while saving must cancel the pending voice start');
    s.elements['mic-btn'].dispatch('mousedown'); await flush();
    assert(s.count('start_voice_input') === 1, 'Voice should start after browser preferences are saved');

    report('PASS: browser auto-save, ordered writes, task/voice gating, stale loads, failure recovery, and missing browser display');
  }
  run().catch(function (error) {
    report(error.stack || String(error));
    if (typeof process !== 'undefined') process.exitCode = 1;
    else quit(1);
  });
})();
