// Run from the repo root with Node, or JavaScriptCore's jsc executable.
// Exercises the in-app browser pane: instant task start (no browser gating),
// auto-show on browser steps, address-bar navigation, and manual toggle.
(function () {
  'use strict';
  var source = typeof require === 'function'
    ? require('node:fs').readFileSync('frontend/app.js', 'utf8')
    : readFile('frontend/app.js');
  var report = typeof print === 'function' ? print : console.log;
  function assert(condition, message) { if (!condition) throw new Error(message); }
  async function flush() { for (var i = 0; i < 30; i++) await Promise.resolve(); }

  function Element(tag) {
    this.tagName = tag;
    this.children = [];
    this.listeners = {};
    this.dataset = {};
    this.style = { setProperty: function () {} };
    this.classList = { add: function () {}, remove: function () {}, toggle: function () {} };
    this.textContent = '';
    this.clientWidth = 0;
    this.clientHeight = 0;
    this._value = '';
    this.hidden = false;
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
  Element.prototype.dispatch = function (event, payload) {
    (this.listeners[event] || []).forEach(function (listener) { listener(payload || { preventDefault: function () {} }); });
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
  Element.prototype.getBoundingClientRect = function () { return { left: 0, top: 0, right: 1000, width: 1000 }; };
  Element.prototype.removeAttribute = function () {};
  Element.prototype.setAttribute = function () {};
  Element.prototype.contains = function () { return false; };

  function boot(saved, opts) {
    opts = opts || {};
    var ensureCalls = 0;
    var elements = {};
    var selects = ['voice-mic', 'ob-default', 'code-default', 'mem-kind'];
    var store = { 'comrade-browser-width': '45' };
    var document = {
      body: new Element('body'),
      documentElement: new Element('html'),
      activeElement: null,
      getElementById: function (id) { return elements[id] || (elements[id] = new Element(selects.indexOf(id) >= 0 ? 'select' : 'div')); },
      createElement: function (tag) { return new Element(tag); },
      querySelectorAll: function () { return []; },
      querySelector: function () { return null; },
      addEventListener: function () {},
    };
    document.getElementById('settings').hidden = true;
    document.getElementById('browser-pane').hidden = true;
    document.getElementById('browser-divider').hidden = true;
    var state = { elements: elements, calls: [], saved: saved || null, agentHandlers: [] };
    function copy(value) { return JSON.parse(JSON.stringify(value)); }
    globalThis.document = document;
    globalThis.requestAnimationFrame = function () {};
    globalThis.window = {
      localStorage: {
        getItem: function (k) { return Object.prototype.hasOwnProperty.call(store, k) ? store[k] : null; },
        setItem: function (k, v) { store[k] = String(v); },
        removeItem: function (k) { delete store[k]; },
      },
      __TAURI__: {
        event: { listen: function (name, fn) { state.agentHandlers.push({ name: name, fn: fn }); } },
        core: { invoke: function (command, args) {
          state.calls.push({ command: command, args: copy(args || {}) });
          if (command === 'app_info') return Promise.resolve({ onboarded: true, memory_kinds: [] });
          if (command === 'get_prefs') return Promise.resolve({ browser: { auto_show: true, width_pct: 45 }, voice: {}, coding: { agents: [], default: '' } });
          if (command === 'save_prefs') { state.saved = copy(args.prefs); return Promise.resolve(state.saved); }
          if (command === 'get_audio_devices') return Promise.resolve({ inputs: [] });
          if (command === 'voice_models_status') return Promise.resolve({ ready: true });
          if (command === 'send_message') return Promise.resolve({ status: 'done' });
          if (command === 'browser_state') return Promise.resolve({ running: true, url: 'https://example.com', title: 'Example' });
          if (command === 'browser_screenshot') return Promise.resolve('data:image/png;base64,AAA');
          if (command === 'browser_open') return Promise.resolve({ url: args.url, title: 'T' });
          if (command === 'browser_frame') return Promise.resolve({ data_url: 'data:image/png;base64,AAA', width: 1280, height: 860 });
          if (command === 'browser_click_at') return Promise.resolve({ url: 'https://example.com' });
          if (command === 'browser_type_text') return Promise.resolve({ typed: (args.text || '').length });
          if (command === 'browser_press_key') return Promise.resolve({ key: args.key });
          if (command === 'browser_scroll') return Promise.resolve({ scrollX: 0, scrollY: 100 });
          if (command === 'browser_back' || command === 'browser_forward' || command === 'browser_reload') {
            return Promise.resolve({ url: 'https://example.com' });
          }
          if (command === 'browser_ensure') {
            ensureCalls++;
            if (opts.ensureFails && ensureCalls <= opts.ensureFails) {
              return Promise.reject('BROWSER_SETUP: installing built-in browser…');
            }
            return Promise.resolve({ running: true, url: 'https://example.com', title: 'Example' });
          }
          if (command === 'browser_provision_status') {
            return Promise.resolve(opts.provisionDone
              ? { installed: true, phase: 'done', downloaded: 100, total: 100, error: '' }
              : { installed: false, phase: 'downloading', downloaded: 50, total: 100, error: '' });
          }
          return Promise.resolve([]);
        } },
      },
    };
    var streamSource = typeof require === 'function' ? require('node:fs').readFileSync('frontend/browser-stream.js', 'utf8') : readFile('frontend/browser-stream.js');
    var inputSource = typeof require === 'function' ? require('node:fs').readFileSync('frontend/browser-input.js', 'utf8') : readFile('frontend/browser-input.js');
    (0, eval)(inputSource.replace('export function', 'function') + '\n' + streamSource.replace('export function createBrowserStream', 'function createBrowserStream') + '\n' + source.replace(/import .*?;\n/g, '').replace('export function initializeComrade()', 'function initializeComrade()') + '\ninitializeComrade();');
    state.fire = function (name, payload) {
      state.agentHandlers.filter(function (h) { return h.name === name; }).forEach(function (h) { h.fn({ payload: payload }); });
    };
    state.count = function (command) { return state.calls.filter(function (c) { return c.command === command; }).length; };
    return state;
  }

  async function run() {
    // 1. Tasks start immediately — no browser prefs gating anymore.
    var s = await boot();
    await flush();
    s.elements.input.value = 'open youtube';
    s.elements.composer.dispatch('submit');
    await flush();
    assert(s.count('send_message') === 1, 'Message must send immediately with no browser setup gating');
    assert(s.count('save_prefs') === 0, 'Sending a message must not write prefs');

    // 2. Browser mode: a browser.* step auto-shows the in-app pane and loads the view.
    s = await boot();
    await flush();
    assert(s.elements['browser-pane'].hidden === true, 'Pane starts hidden');
    s.fire('agent-event', { type: 'step', label: 'browser.open url="https://example.com"', status: 'done' });
    await flush();
    assert(s.elements['browser-pane'].hidden === false, 'Browser step must auto-show the in-app pane');
    assert(s.count('browser_state') >= 1, 'Pane must fetch browser state on auto-show');
    assert(s.count('browser_frame') >= 1, 'Pane must render a live frame on auto-show');

    // 3. Address bar drives the same bundled tab; non-browser steps don't touch the pane.
    var shots = s.count('browser_frame');
    s.elements['browser-url'].value = 'example.org';
    s.elements['browser-form'].dispatch('submit');
    await flush();
    assert(s.count('browser_open') === 1, 'Address bar must navigate via browser_open');
    assert(s.calls.filter(function (c) { return c.command === 'browser_open'; })[0].args.url === 'example.org',
      'Address bar must send the typed URL to the bundled tab');
    s.fire('agent-event', { type: 'step', label: 'terminal.execute command="ls"', status: 'done' });
    await flush();
    assert(s.count('browser_frame') === shots + 1, 'Only the address-bar refresh may add exactly one frame');
    s.fire('agent-event', { type: 'done', status: 'done' });
    await flush();

    // 4. Manual toggle hides/shows the pane inside the app.
    s.elements['browser-btn'].dispatch('click');
    await flush();
    assert(s.elements['browser-pane'].hidden === true, 'Toggle must hide the pane');
    s.elements['browser-btn'].dispatch('click');
    await flush();
    assert(s.elements['browser-pane'].hidden === false, 'Toggle must re-open the pane');

    // 5. First-run self-install: pane opens, shows install progress, no dead-end error.
    s = await boot(null, { ensureFails: 99 });
    await flush();
    s.elements['browser-btn'].dispatch('click');
    await flush();
    assert(s.elements['browser-pane'].hidden === false, 'Pane must open even while installing');
    assert(s.elements['browser-status'].textContent.indexOf('Installing built-in browser') === 0,
      'Pane must show install progress, got: ' + s.elements['browser-status'].textContent);
    assert(s.count('browser_provision_status') === 0, 'Progress polls on a timer, not in a burst');

    // 6. Direct interaction: clicks map onto page coords, keys and wheel forward.
    s = await boot();
    await flush();
    s.elements['browser-btn'].dispatch('click');
    await flush();
    assert(s.elements['browser-pane'].hidden === false, 'Pane must open for interaction');
    s.elements['browser-img'].clientWidth = 640;
    s.elements['browser-img'].dispatch('click', { clientX: 320, clientY: 160, offsetX: 320, offsetY: 160, preventDefault: function () {} });
    await flush();
    var clicks = s.calls.filter(function (c) { return c.command === 'browser_click_at'; });
    assert(clicks.length === 1, 'Clicking the view must click the page');
    assert(clicks[0].args.x === 640 && clicks[0].args.y === 320,
      'Clicks must scale to page coords, got ' + JSON.stringify(clicks[0].args));
    s.elements['browser-keys'].value = 'hi';
    s.elements['browser-keys'].dispatch('input');
    await flush();
    var types = s.calls.filter(function (c) { return c.command === 'browser_type_text'; });
    assert(types.length === 1 && types[0].args.text === 'hi', 'Typing must forward text to the page');
    s.elements['browser-keys'].dispatch('keydown', { key: 'Enter', preventDefault: function () {} });
    await flush();
    assert(s.count('browser_press_key') === 1, 'Special keys must forward to the page');
    s.elements['browser-view'].dispatch('wheel', { deltaX: 0, deltaY: 200, preventDefault: function () {} });
    await new Promise(function (r) { setTimeout(r, 250); });
    assert(s.count('browser_scroll') === 1, 'Wheel must scroll the page');

    report('PASS: instant task start, browser-mode auto-show, address-bar navigation, pane toggle, self-install progress, direct interaction');
    // Install-progress polling uses a real timer that the mock never
    // completes — exit explicitly instead of hanging on it.
    if (typeof process !== 'undefined') process.exit(0);
  }
  run().catch(function (error) {
    report(error.stack || String(error));
    if (typeof process !== 'undefined') process.exit(1);
    else quit(1);
  });
})();
