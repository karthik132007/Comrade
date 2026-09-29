/* Comrade frontend (vanilla JS). Talks to the Rust backend via Tauri IPC only. */
(function () {
  'use strict';

  var tauri = (window.__TAURI__ && window.__TAURI__.core) || null;
  var tauriEvent = (window.__TAURI__ && window.__TAURI__.event) || null;

  var pill = document.getElementById('status-pill');
  var statusText = document.getElementById('status-text');
  var orb = document.getElementById('orb');
  var waveEl = document.getElementById('wave');
  var waveBars = [];
  var waveLevel = 0;      // target 0..1 from mic
  var waveShown = 0;      // smoothed displayed level
  var uiState = 'idle';
  (function buildWave() {
    for (var i = 0; i < 28; i++) {
      var b = document.createElement('i');
      waveEl.appendChild(b);
      waveBars.push(b);
    }
    var t = 0;
    function frame() {
      t += 0.06;
      var active = uiState === 'listening' || uiState === 'speaking' || uiState === 'interrupted';
      waveShown += ((active ? Math.max(waveLevel, uiState === 'speaking' ? 0.35 : 0) : 0) - waveShown) * 0.25;
      for (var i = 0; i < waveBars.length; i++) {
        var shimmer = 0.55 + 0.45 * Math.sin(t * (2 + (i % 5) * 0.35) + i * 0.7);
        var h = 3 + waveShown * 34 * shimmer;
        waveBars[i].style.height = Math.round(active ? h : 3) + 'px';
      }
      var hot = waveShown > 0.45;
      waveEl.className = uiState === 'speaking' ? 'speaking' : (hot ? 'hot' : '');
      requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  })();
  var greeting = document.getElementById('greeting');
  var subtitle = document.getElementById('subtitle');
  var form = document.getElementById('composer');
  var input = document.getElementById('input');
  var messagesEl = document.getElementById('messages');
  var taskPanel = document.getElementById('task-panel');
  var taskTitle = document.getElementById('task-title');
  var stepsEl = document.getElementById('steps');
  var cancelBtn = document.getElementById('cancel-btn');
  var micBtn = document.getElementById('mic-btn');
  var convBtn = document.getElementById('conv-btn');
  var permModal = document.getElementById('perm-modal');
  var permSummary = document.getElementById('perm-summary');
  var permAllow = document.getElementById('perm-allow');
  var permCancel = document.getElementById('perm-cancel');

  var currentResponseEl = null;
  var pendingPermId = null;
  var taskStartVersion = 0;
  var messageStarting = false;

  // --- chat history sidebar (history.db sessions) ---
  var sidebarEl = document.getElementById('sidebar');
  var sidebarBtn = document.getElementById('sidebar-btn');
  var newChatBtn = document.getElementById('new-chat-btn');
  var chatList = document.getElementById('chat-list');
  var currentSessionId = null;
  try { currentSessionId = window.localStorage.getItem('comrade-session') || null; } catch (e) { /* noop */ }

  function setSession(id) {
    currentSessionId = id || null;
    try {
      if (currentSessionId) window.localStorage.setItem('comrade-session', currentSessionId);
      else window.localStorage.removeItem('comrade-session');
    } catch (e) { /* noop */ }
  }

  var STATE_LABEL = {
    idle: 'READY',
    listening: 'LISTENING',
    thinking: 'THINKING',
    executing: 'EXECUTING',
    speaking: 'SPEAKING',
    interrupted: 'INTERRUPTED',
    error: 'ERROR',
  };

  function invoke(cmd, args) {
    if (!tauri) return Promise.reject(new Error('Tauri backend not available.'));
    return tauri.invoke(cmd, args || {});
  }

  function setState(state) {
    var s = STATE_LABEL[state] ? state : 'idle';
    uiState = s;
    pill.className = 'pill ' + s;
    statusText.textContent = STATE_LABEL[s];
    orb.className = 'orb ' + s;
  }

  function syncConvClass() {
    document.body.classList.toggle('conv-on', convMode);
  }

  function addMessage(role, text) {
    var div = document.createElement('div');
    div.className = 'msg ' + role;
    var r = document.createElement('div');
    r.className = 'role';
    r.textContent = role === 'user' ? 'YOU' : 'COMRADE';
    var b = document.createElement('div');
    b.textContent = text;
    div.appendChild(r);
    div.appendChild(b);
    messagesEl.appendChild(div);
    div.scrollIntoView({ block: 'end' });
    return b;
  }

  function resetTaskPanel(title) {
    taskPanel.hidden = false;
    taskTitle.textContent = title;
    stepsEl.innerHTML = '';
    greeting.textContent = 'Working on it.';
    subtitle.textContent = title;
  }

  function upsertStep(label, status, detail) {
    var li = document.getElementById('step-' + hash(label));
    if (!li) {
      li = document.createElement('li');
      li.id = 'step-' + hash(label);
      stepsEl.appendChild(li);
    }
    li.className = status;
    var icon = status === 'done' ? '✓ ' : status === 'failed' ? '✗ ' : '→ ';
    li.textContent = icon + label + (detail ? ' — ' + detail : '');
  }

  function hash(s) {
    var h = 0;
    for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
    return String(Math.abs(h));
  }

  form.addEventListener('submit', function (ev) {
    ev.preventDefault();
    var text = input.value.trim();
    if (!text || messageStarting) return;
    messageStarting = true;
    if (input.value.trim() === text) input.value = '';
    addMessage('user', text);
    resetTaskPanel(text);
    currentResponseEl = null;
    invoke('send_message', { text: text, sessionId: currentSessionId }).then(function (res) {
      if (!res) return;
      if (res.session_id) { setSession(res.session_id); refreshChatList(); }
      if (res.status === 'done' || res.status === 'running') return;
      setState('idle');
      if (res.error) addMessage('comrade', 'Error: ' + res.error);
    }).catch(function (err) {
      messageStarting = false;
      setState('error');
      addMessage('comrade', 'Error: ' + (err && err.message ? err.message : err));
    });
  });

  cancelBtn.addEventListener('click', function () {
    taskStartVersion++;
    invoke('cancel_voice_input', {}).catch(function () { /* noop */ });
    voiceActive = false;
    convMode = false;
    syncConvClass();
    if (typeof convBtn !== 'undefined' && convBtn) convBtn.classList.remove('live');
    micBtn.classList.remove('live');
    invoke('cancel_task').catch(function () { /* noop */ });
    setState('idle');
    greeting.textContent = 'Cancelled.';
    subtitle.textContent = 'How can I help you?';
  });

  // --- local voice sessions (backend mic/VAD/STT/TTS; no audio in the UI) ---
  var voiceActive = false;
  var voiceStartPending = false;
  var voiceStartVersion = 0;
  var convMode = false;

  function startVoice(oneShot) {
    if (voiceActive) return;
    voiceActive = true;
    voiceStartPending = true;
    var startVersion = ++voiceStartVersion;
    micBtn.classList.add('live');
    currentResponseEl = null;
    voiceStartPending = false;
    invoke('start_voice_input', { oneShot: oneShot, sessionId: currentSessionId }).then(function (res) {
      if (startVersion !== voiceStartVersion) return;
      voiceStartPending = false;
      if (res === 'started') return;
      voiceActive = false;
      micBtn.classList.remove('live');
    }).catch(function (err) {
      if (startVersion !== voiceStartVersion) return;
      voiceStartPending = false;
      voiceActive = false;
      micBtn.classList.remove('live');
      convMode = false;
      syncConvClass();
      convBtn.classList.remove('live');
      setState('error');
      addMessage('comrade', 'Voice: ' + errMsg(err));
    });
  }

  function releaseVoice() {
    // Push-to-talk release: finalize the utterance, hear the reply, end.
    if (!voiceActive) return;
    micBtn.classList.remove('live');
    if (voiceStartPending) {
      voiceActive = false;
      voiceStartPending = false;
      voiceStartVersion++;
      return;
    }
    invoke('stop_voice_input', { finalize: true }).catch(function () { /* session ends on its own */ });
  }

  function toggleConversation() {
    if (convMode) {
      convMode = false;
      syncConvClass();
      convBtn.classList.remove('live');
      invoke('stop_voice_input', { finalize: false }).catch(function () { /* noop */ });
      voiceActive = false;
      micBtn.classList.remove('live');
      return;
    }
    convMode = true;
    convBtn.classList.add('live');
    syncConvClass();
    startVoice(false);
  }

  micBtn.addEventListener('mousedown', function (ev) { ev.preventDefault(); startVoice(true); });
  micBtn.addEventListener('mouseup', function () { releaseVoice(); });
  micBtn.addEventListener('mouseleave', function () { if (voiceActive && !convMode) releaseVoice(); });
  micBtn.addEventListener('touchstart', function (ev) { ev.preventDefault(); startVoice(true); }, { passive: false });
  micBtn.addEventListener('touchend', function (ev) { ev.preventDefault(); releaseVoice(); }, { passive: false });
  convBtn.addEventListener('click', toggleConversation);

  function onVoiceEvent(ev) {
    if (ev.type === 'voice-partial') {
      setState('listening');
      subtitle.textContent = ev.text || 'Listening...';
    } else if (ev.type === 'voice-models') {
      modelStatus.textContent = ev.ready
        ? 'models ready (offline)'
        : 'missing: ' + (ev.missing || []).join(', ');
      if (ev.ready) {
        modelProgress.style.width = '100%';
        setModelDownloadActive(false);
      } else if (!modelDownloadActive) {
        modelProgress.style.width = '0';
      }
    } else if (ev.type === 'voice-download') {
      setModelDownloadActive(true);
      if (ev.file === 'extracting') {
        modelStatus.textContent = 'extracting ' + ev.pack + ' model...';
        modelProgress.style.width = '0';
        return;
      }
      var total = ev.total ? ' / ' + Math.round(ev.total / 1024) + 'KB' : '';
      var pack = ev.pack ? ev.pack + ' · ' : '';
      modelStatus.textContent = 'downloading ' + pack + ev.file + ': ' + Math.round(ev.downloaded / 1024) + 'KB' + total;
      var pct = ev.total ? Math.round(100 * ev.downloaded / ev.total) : 0;
      modelProgress.style.width = pct + '%';
    } else if (ev.type === 'voice-download-error') {
      modelStatus.textContent = 'download failed: ' + (ev.message || 'unknown error');
      modelProgress.style.width = '0';
      setModelDownloadActive(false);
      addMessage('comrade', 'Voice model download failed: ' + (ev.message || 'unknown error'));
    } else if (ev.type === 'voice-level') {
      waveLevel = Math.max(0, Math.min(1, ev.level || 0));
    } else if (ev.type === 'voice-error') {
      setState('error');
      addMessage('comrade', 'Voice: ' + ev.message);
    }
  }

  function onAgentEvent(ev) {
    if (ev.type === 'state') {
      setState(ev.state);
      if (ev.state === 'thinking') subtitle.textContent = 'Understanding request...';
      if (ev.state === 'listening') subtitle.textContent = 'Listening...';
      if (ev.state === 'speaking') subtitle.textContent = 'Speaking...';
      if (ev.state === 'interrupted') subtitle.textContent = 'Interrupted — listening...';
      if (ev.state === 'idle') {
        greeting.textContent = 'Hey, Comrade.';
        subtitle.textContent = 'How can I help you?';
      }
    } else if (ev.type === 'transcript') {
      addMessage('user', ev.transcript);
      resetTaskPanel(ev.transcript);
      currentResponseEl = null;
    } else if (ev.type === 'step') {
      upsertStep(ev.label, ev.status, ev.detail);
      if (ev.label && ev.label.indexOf('browser.') === 0) onBrowserActivity();
    } else if (ev.type === 'token') {
      if (!currentResponseEl) currentResponseEl = addMessage('comrade', '');
      currentResponseEl.textContent += ev.token;
    } else if (ev.type === 'done') {
      setState(ev.status === 'done' ? 'idle' : 'error');
      if (browserIsOpen) refreshBrowser();
      if (ev.status !== 'done' && ev.error) addMessage('comrade', 'Error: ' + ev.error);
    }
  }

  function onPermissionRequest(req) {
    pendingPermId = req.id;
    permSummary.textContent = req.summary;
    permModal.hidden = false;
  }

  if (tauriEvent) {
    tauriEvent.listen('agent-event', function (e) { onAgentEvent(e.payload); });
    tauriEvent.listen('voice-event', function (e) { onVoiceEvent(e.payload); });
    tauriEvent.listen('permission-request', function (e) { onPermissionRequest(e.payload); });
  } else {
    addMessage('comrade', 'Backend bridge unavailable (not running inside Comrade).');
  }

  function answerPerm(approved) {
    if (!pendingPermId) return;
    invoke('permission_response', { id: pendingPermId, approved: approved }).catch(function () { /* noop */ });
    pendingPermId = null;
    permModal.hidden = true;
  }

  permAllow.addEventListener('click', function () { answerPerm(true); });
  permCancel.addEventListener('click', function () { answerPerm(false); });

  /* --- settings side panel: memory management --- */

  var settingsEl = document.getElementById('settings');
  var settingsBtn = document.getElementById('settings-btn');
  var settingsClose = document.getElementById('settings-close');
  var modelInfo = document.getElementById('model-info');
  var memText = document.getElementById('mem-text');
  var memKind = document.getElementById('mem-kind');
  var memSave = document.getElementById('mem-save');
  var memStatus = document.getElementById('mem-status');
  var importFile = document.getElementById('import-file');
  var importBtn = document.getElementById('import-btn');
  var importStatus = document.getElementById('import-status');
  var voiceMic = document.getElementById('voice-mic');
  var voiceModelsBtn = document.getElementById('voice-models-btn');
  var voiceDlBtn = document.getElementById('voice-dl-btn');
  var voiceVadT = document.getElementById('voice-vad-t');
  var voiceSilence = document.getElementById('voice-silence');
  var voiceTtsVoice = document.getElementById('voice-tts-voice');
  var voiceTtsSpeed = document.getElementById('voice-tts-speed');
  var modelStatus = document.getElementById('model-status');
  var modelProgress = document.getElementById('model-progress');
  var modelDownloadActive = false;
  var currentVoicePrefs = null;
  var memSearch = document.getElementById('mem-search');
  var memRefresh = document.getElementById('mem-refresh');
  var memCount = document.getElementById('mem-count');
  var memList = document.getElementById('mem-list');
  var searchTimer = null;

  function errMsg(err) {
    return (err && err.message ? err.message : String(err)).slice(0, 200);
  }

  function setModelDownloadActive(active) {
    modelDownloadActive = !!active;
    voiceDlBtn.disabled = modelDownloadActive;
    voiceModelsBtn.disabled = modelDownloadActive;
    voiceDlBtn.textContent = modelDownloadActive ? 'Downloading...' : 'Download';
  }

  function openSettings() {
    settingsEl.hidden = false;
    refreshAppInfo();
    refreshMemList();
  }

  settingsBtn.addEventListener('click', function () {
    if (settingsEl.hidden) openSettings();
    else settingsEl.hidden = true;
  });
  settingsClose.addEventListener('click', function () { settingsEl.hidden = true; });

  function refreshAppInfo() {
    invoke('app_info', {}).then(function (info) {
      var kinds = (info.memory_kinds || []).map(function (k) { return k[0] + ':' + k[1]; }).join(' ');
      modelInfo.textContent = info.provider + ' \u00b7 ' + info.model +
        '  |  memories: ' + info.memory_count + (kinds ? ' (' + kinds + ')' : '');
    }).catch(function (err) {
      modelInfo.textContent = 'unavailable: ' + errMsg(err);
    });
  }

  function renderMemList(items) {
    memList.innerHTML = '';
    memCount.textContent = items.length + ' shown';
    items.forEach(function (m) {
      var li = document.createElement('li');
      var head = document.createElement('div');
      var kind = document.createElement('span');
      kind.className = 'kind';
      kind.textContent = m.kind.toUpperCase();
      head.appendChild(kind);
      if (m.source) {
        var src = document.createElement('span');
        src.className = 'src';
        src.textContent = ' \u00b7 ' + m.source;
        head.appendChild(src);
      }
      var p = document.createElement('p');
      p.textContent = m.text.length > 300 ? m.text.slice(0, 300) + '\u2026' : m.text;
      var del = document.createElement('button');
      del.textContent = 'forget';
      del.addEventListener('click', function () {
        invoke('memory_delete', { id: m.id }).then(refreshMemList).catch(function (e) {
          memStatus.textContent = 'delete failed: ' + errMsg(e);
        });
      });
      li.appendChild(head);
      li.appendChild(p);
      li.appendChild(del);
      memList.appendChild(li);
    });
  }

  function refreshMemList() {
    var q = memSearch.value.trim();
    var p = q ? invoke('memory_search', { query: q }) : invoke('memory_list', { limit: 20 });
    p.then(function (items) {
      renderMemList(items || []);
      refreshAppInfo();
    }).catch(function (err) {
      memCount.textContent = 'failed: ' + errMsg(err);
    });
  }

  memSave.addEventListener('click', function () {
    var text = memText.value.trim();
    if (!text) { memStatus.textContent = 'Write something first.'; return; }
    memStatus.textContent = 'saving...';
    invoke('memory_add', { text: text, kind: memKind.value }).then(function (res) {
      memStatus.textContent = 'stored ' + res.stored + ' memor' + (res.stored === 1 ? 'y.' : 'ies.');
      memText.value = '';
      refreshMemList();
    }).catch(function (err) {
      memStatus.textContent = 'failed: ' + errMsg(err);
    });
  });

  importBtn.addEventListener('click', function () {
    var f = importFile.files && importFile.files[0];
    if (!f) { importStatus.textContent = 'Pick a conversations.json file first.'; return; }
    importStatus.textContent = 'reading + embedding (may take a while)...';
    var reader = new FileReader();
    reader.onload = function () {
      invoke('memory_import_chatgpt', { jsonText: String(reader.result || '') }).then(function (res) {
        importStatus.textContent = 'imported ' + res.stored + ' memories from ChatGPT.';
        importFile.value = '';
        refreshMemList();
      }).catch(function (err) {
        importStatus.textContent = 'failed: ' + errMsg(err);
      });
    };
    reader.onerror = function () { importStatus.textContent = 'could not read file.'; };
    reader.readAsText(f);
  });

  memRefresh.addEventListener('click', refreshMemList);

  function fmtDate(ms) {
    try {
      return new Date(ms).toLocaleDateString([], { month: 'short', day: 'numeric' });
    } catch (e) { return ''; }
  }

  function toggleSidebar(force) {
    var show = typeof force === 'boolean' ? force : sidebarEl.hidden;
    sidebarEl.hidden = !show;
    document.body.classList.toggle('sidebar-open', show);
  }

  sidebarBtn.addEventListener('click', function () { toggleSidebar(); });

  function clearMessages() {
    messagesEl.innerHTML = '';
    currentResponseEl = null;
  }

  function newChat() {
    setSession(null);
    clearMessages();
    taskPanel.hidden = true;
    greeting.textContent = 'Hey, Comrade.';
    subtitle.textContent = 'How can I help you?';
    refreshChatList();
    input.focus();
  }

  newChatBtn.addEventListener('click', newChat);

  function renderChatList(sessions) {
    chatList.innerHTML = '';
    (sessions || []).forEach(function (sn) {
      var li = document.createElement('li');
      if (sn.id === currentSessionId) li.className = 'active';
      var del = document.createElement('button');
      del.className = 'chat-del';
      del.textContent = '✕';
      del.title = 'Delete chat';
      del.addEventListener('click', function (ev) {
        ev.stopPropagation();
        invoke('history_delete', { sessionId: sn.id }).then(function () {
          if (sn.id === currentSessionId) newChat();
          else refreshChatList();
        }).catch(function () { /* noop */ });
      });
      var title = document.createElement('div');
      title.className = 'chat-title';
      title.textContent = sn.title || 'New chat';
      var meta = document.createElement('div');
      meta.className = 'chat-meta';
      meta.textContent = fmtDate(sn.updated_at) + ' · ' + sn.message_count + ' msgs';
      li.appendChild(del);
      li.appendChild(title);
      li.appendChild(meta);
      li.addEventListener('click', function () { loadSession(sn.id, sn.title); });
      chatList.appendChild(li);
    });
  }

  function refreshChatList() {
    invoke('history_list', { limit: 50 }).then(function (sessions) {
      renderChatList(sessions);
    }).catch(function () { /* sidebar stays as-is when backend is away */ });
  }

  function loadSession(id, title) {
    invoke('history_get', { sessionId: id }).then(function (msgs) {
      setSession(id);
      clearMessages();
      taskPanel.hidden = true;
      greeting.textContent = 'History';
      subtitle.textContent = title || '';
      (msgs || []).forEach(function (m) {
        addMessage(m.role === 'user' ? 'user' : 'comrade', m.content);
      });
      refreshChatList();
    }).catch(function (err) {
      addMessage('comrade', 'Error loading chat: ' + errMsg(err));
    });
  }

  /* --- in-app browser pane (bundled Chromium, live screenshots) ---
     The agent drives exactly one browser — Comrade's own bundled Chromium,
     always headless so nothing opens outside the app. This pane is its only
     visible surface: a live screenshot view with an address bar, resizable
     via the divider, all inside the app. */
  var browserBtn = document.getElementById('browser-btn');
  var browserPane = document.getElementById('browser-pane');
  var browserDivider = document.getElementById('browser-divider');
  var browserForm = document.getElementById('browser-form');
  var browserUrl = document.getElementById('browser-url');
  var browserImg = document.getElementById('browser-img');
  var browserView = document.getElementById('browser-view');
  var browserTitle = document.getElementById('browser-title');
  var browserStatus = document.getElementById('browser-status');
  var browserBackBtn = document.getElementById('browser-back');
  var browserFwdBtn = document.getElementById('browser-forward');
  var browserReloadBtn = document.getElementById('browser-reload');
  var browserShotBtn = document.getElementById('browser-shot');
  var browserHideBtn = document.getElementById('browser-hide');
  var browserIsOpen = false;
  var browserAutoShow = true;
  var browserRefreshing = false;

  function clampWidth(pct) {
    pct = Math.round(Number(pct) || 45);
    return Math.max(20, Math.min(70, pct));
  }

  function applyBrowserWidth(pct, persist) {
    pct = clampWidth(pct);
    document.documentElement.style.setProperty('--browser-w', pct + '%');
    try { window.localStorage.setItem('comrade-browser-width', String(pct)); } catch (e) { /* noop */ }
    var slider = document.getElementById('pref-bwidth');
    var val = document.getElementById('pref-bwidth-val');
    if (slider) slider.value = String(pct);
    if (val) val.textContent = pct + '%';
    if (persist) persistBrowserPrefs();
    return pct;
  }

  function setBrowserOpen(open, refresh) {
    browserIsOpen = !!open;
    document.body.classList.toggle('browser-open', browserIsOpen);
    if (browserPane) browserPane.hidden = !browserIsOpen;
    if (browserDivider) browserDivider.hidden = !browserIsOpen;
    try { window.localStorage.setItem('comrade-browser-open', browserIsOpen ? '1' : '0'); } catch (e) { /* noop */ }
    if (browserIsOpen && refresh !== false) ensureBrowserReady();
  }

  // Browser self-install: the built-in Chromium downloads itself once
  // (first launch / first use). While that runs, the pane shows live %.
  var browserInstallTimer = null;

  function fmtBytes(n) {
    n = Number(n) || 0;
    if (n >= 1048576) return (n / 1048576).toFixed(1) + 'MB';
    if (n >= 1024) return Math.round(n / 1024) + 'KB';
    return n + 'B';
  }

  function stopInstallPoll() {
    if (browserInstallTimer) { clearInterval(browserInstallTimer); browserInstallTimer = null; }
  }

  function pollBrowserInstall(done) {
    stopInstallPoll();
    browserInstallTimer = setInterval(function () {
      invoke('browser_provision_status', {}).then(function (st) {
        st = st || {};
        if (st.installed) { stopInstallPoll(); done(true); return; }
        if (st.phase === 'failed') {
          stopInstallPoll();
          if (browserStatus) browserStatus.textContent = 'Install failed: ' + String(st.error || 'unknown error').slice(0, 160);
          done(false);
          return;
        }
        var label = 'Installing built-in browser (one-time)';
        if (st.phase === 'resolving') label += ' — finding latest release…';
        else if (st.phase === 'extracting') label += ' — unpacking…';
        else if (st.total) label += ' — ' + Math.round(100 * st.downloaded / st.total) + '% (' + fmtBytes(st.downloaded) + ' / ' + fmtBytes(st.total) + ')';
        else if (st.downloaded) label += ' — ' + fmtBytes(st.downloaded) + '…';
        else label += '…';
        if (browserStatus) browserStatus.textContent = label;
      }).catch(function () { /* keep polling */ });
    }, 1000);
  }

  function isInstallError(msg) {
    return /BROWSER_SETUP|install|download|connection|network|offline|release index/i.test(msg || '');
  }

  // Ensure the built-in browser is installed and running, showing install
  // progress in the pane. Resolves true when the live view is refreshing.
  function ensureBrowserReady() {
    if (browserStatus) browserStatus.textContent = 'Starting built-in browser…';
    return invoke('browser_ensure', {}).then(function (st) {
      stopInstallPoll();
      st = st || {};
      if (st.url && browserUrl) browserUrl.value = st.url;
      if (st.title && browserTitle) browserTitle.textContent = st.title + (st.url ? ' — ' + st.url : '');
      refreshBrowser();
      return true;
    }).catch(function (err) {
      var msg = errMsg(err);
      if (!isInstallError(msg)) {
        if (browserStatus) browserStatus.textContent = 'Browser unavailable: ' + msg;
        return false;
      }
      if (browserStatus) browserStatus.textContent = 'Installing built-in browser (one-time)…';
      return new Promise(function (resolve) {
        pollBrowserInstall(function (ok) {
          if (ok) ensureBrowserReady().then(resolve);
          else resolve(false);
        });
      });
    });
  }

  function refreshBrowser() {
    if (browserRefreshing) return;
    browserRefreshing = true;
    if (browserStatus) browserStatus.textContent = 'updating…';
    invoke('browser_state', {}).then(function (st) {
      st = st || {};
      if (st.url && browserUrl && document.activeElement !== browserUrl) browserUrl.value = st.url;
      if (browserTitle) browserTitle.textContent = st.title ? st.title + ' — ' + (st.url || '') : (st.url || 'Comrade\u2019s browser — shown only here, inside the app.');
      if (!st.running) {
        if (browserStatus) browserStatus.textContent = 'Browser is idle. Open a page or run a task.';
        if (browserView) browserView.classList.add('idle');
        return null;
      }
      if (browserView) browserView.classList.remove('idle');
      return invoke('browser_screenshot', {});
    }).then(function (dataUrl) {
      if (dataUrl && browserImg) browserImg.src = dataUrl;
      if (browserStatus) browserStatus.textContent = '';
    }).catch(function (err) {
      var msg = errMsg(err);
      if (isInstallError(msg)) { ensureBrowserReady(); return; }
      if (browserStatus) browserStatus.textContent = 'Browser unavailable: ' + msg;
    }).then(function () {
      browserRefreshing = false;
    });
  }

  // Browser mode: auto-show the pane when the agent touches the browser.
  function onBrowserActivity() {
    if (browserAutoShow && !browserIsOpen) setBrowserOpen(true);
    else if (browserIsOpen) refreshBrowser();
    else if (browserStatus) browserStatus.textContent = 'Agent is browsing (pane hidden).';
  }

  if (browserBtn) browserBtn.addEventListener('click', function () { setBrowserOpen(!browserIsOpen); });
  if (browserHideBtn) browserHideBtn.addEventListener('click', function () { setBrowserOpen(false); });
  if (browserShotBtn) browserShotBtn.addEventListener('click', refreshBrowser);
  if (browserBackBtn) browserBackBtn.addEventListener('click', function () {
    invoke('browser_back', {}).then(function (r) {
      if (r && r.url && browserUrl) browserUrl.value = r.url;
      refreshBrowser();
    }).catch(function (err) { if (browserStatus) browserStatus.textContent = 'Back failed: ' + errMsg(err); });
  });
  if (browserFwdBtn) browserFwdBtn.addEventListener('click', function () {
    invoke('browser_forward', {}).then(function (r) {
      if (r && r.url && browserUrl) browserUrl.value = r.url;
      refreshBrowser();
    }).catch(function (err) { if (browserStatus) browserStatus.textContent = 'Forward failed: ' + errMsg(err); });
  });
  if (browserReloadBtn) browserReloadBtn.addEventListener('click', function () {
    invoke('browser_reload', {}).then(refreshBrowser).catch(function (err) {
      if (browserStatus) browserStatus.textContent = 'Reload failed: ' + errMsg(err);
    });
  });
  if (browserForm) browserForm.addEventListener('submit', function (ev) {
    ev.preventDefault();
    var url = browserUrl.value.trim();
    if (!url) return;
    if (browserStatus) browserStatus.textContent = 'loading…';
    invoke('browser_open', { url: url }).then(function (r) {
      if (r && r.url && browserUrl) browserUrl.value = r.url;
      refreshBrowser();
    }).catch(function (err) {
      if (isInstallError(errMsg(err))) { ensureBrowserReady(); return; }
      if (browserStatus) browserStatus.textContent = 'Open failed: ' + errMsg(err);
    });
  });

  // Drag the divider to resize the pane (in-app only).
  (function wireDivider() {
    if (!browserDivider) return;
    var dragging = false;
    browserDivider.addEventListener('mousedown', function (ev) {
      ev.preventDefault();
      dragging = true;
      browserDivider.classList.add('drag');
    });
    document.addEventListener('mousemove', function (ev) {
      if (!dragging) return;
      var rect = document.getElementById('workarea').getBoundingClientRect();
      var pct = 100 * (rect.right - ev.clientX) / Math.max(rect.width, 1);
      applyBrowserWidth(pct, false);
    });
    document.addEventListener('mouseup', function () {
      if (!dragging) return;
      dragging = false;
      browserDivider.classList.remove('drag');
      var slider = document.getElementById('pref-bwidth');
      applyBrowserWidth(slider ? slider.value : 45, true);
    });
  })();

  // Restore pane size/open state from the last session immediately.
  try {
    var savedW = window.localStorage.getItem('comrade-browser-width');
    if (savedW) applyBrowserWidth(savedW, false);
    else applyBrowserWidth(45, false);
  } catch (e) { /* noop */ }

  /* --- onboarding + preferences (comrade.conf) --- */
  var onboardingEl = document.getElementById('onboarding');
  var obContinue = document.getElementById('ob-continue');
  var obStatus = document.getElementById('ob-status');
  var prefAutoshow = document.getElementById('pref-autoshow');
  var prefAutoplay = document.getElementById('pref-autoplay');
  var prefSave = document.getElementById('pref-save');
  var prefStatus = document.getElementById('pref-status');
  var prefFile = document.getElementById('pref-file');
  var prefBwidth = document.getElementById('pref-bwidth');
  var prefBwidthVal = document.getElementById('pref-bwidth-val');
  var paneOpenBtn = document.getElementById('pane-open-btn');
  var paneShotBtn = document.getElementById('pane-shot-btn');
  var paneCloseBtn = document.getElementById('pane-close-btn');
  var panePrefsTimer = null;

  // Persist pane prefs (auto_show, width_pct) without touching the rest.
  function persistBrowserPrefs() {
    if (panePrefsTimer) clearTimeout(panePrefsTimer);
    panePrefsTimer = setTimeout(function () {
      var width = clampWidth(prefBwidth ? prefBwidth.value : 45);
      var auto = prefAutoshow ? prefAutoshow.checked : true;
      invoke('get_prefs', {}).then(function (existing) {
        existing = existing || {};
        existing.browser = { auto_show: auto, width_pct: width };
        return invoke('save_prefs', { prefs: existing });
      }).then(function () {
        if (prefStatus) prefStatus.textContent = 'Browser pane settings saved.';
      }).catch(function (err) {
        if (prefStatus) prefStatus.textContent = 'Browser pane not saved: ' + errMsg(err);
      });
    }, 400);
  }

  if (prefAutoshow) prefAutoshow.addEventListener('change', function () {
    browserAutoShow = prefAutoshow.checked;
    persistBrowserPrefs();
  });
  if (prefBwidth) prefBwidth.addEventListener('input', function () {
    applyBrowserWidth(prefBwidth.value, false);
  });
  if (prefBwidth) prefBwidth.addEventListener('change', function () {
    applyBrowserWidth(prefBwidth.value, true);
  });
  if (paneOpenBtn) paneOpenBtn.addEventListener('click', function () { setBrowserOpen(true); });
  if (paneShotBtn) paneShotBtn.addEventListener('click', function () { setBrowserOpen(true); refreshBrowser(); });
  if (paneCloseBtn) paneCloseBtn.addEventListener('click', function () {
    invoke('browser_close', {}).then(function () {
      if (browserStatus) browserStatus.textContent = 'Browser stopped.';
      if (browserImg) browserImg.removeAttribute('src');
      if (browserView) browserView.classList.add('idle');
      refreshBrowser();
    }).catch(function (err) { if (browserStatus) browserStatus.textContent = 'Stop failed: ' + errMsg(err); });
  });

  obContinue.addEventListener('click', function () {
    var agents = checkedAgentIds(agentList);
    if (!agents.length) { obStatus.textContent = 'Enable at least one coding agent.'; return; }
    obStatus.textContent = 'saving...';
    invoke('get_prefs', {}).then(function (existing) {
      var v = (existing && existing.voice) || {};
      return invoke('save_prefs', { prefs: {
        browser: (existing && existing.browser) || { auto_show: true, width_pct: 45 },
        voice: {
          autoplay: true, enabled: true, mic: v.mic || '',
          stt: v.stt || { engine: 'sherpa-onnx', model: 'zipformer-en-20M-int8', language: 'en', sample_rate: 16000 },
          vad: v.vad || { threshold: 0.5, silence_ms: 700, min_speech_ms: 250 },
          tts: v.tts || { engine: 'kokoro', voice: '0', speed: 1.0 },
          runtime: v.runtime || { max_utterance_ms: 30000, decode_every_frames: 16, chunk_max_chars: 220, chunk_min_merge: 12, num_threads: 2 },
        },
        coding: { agents: agents, default: obDefault.value || agents[0] },
        llm: (existing && existing.llm) || { provider: 'deepseek', model: 'deepseek-flash' },
        agent: (existing && existing.agent) || { max_steps: 15, timeout_ms: 120000 },
        memory: (existing && existing.memory) || { embedding_model: 'openai/text-embedding-3-small', embedding_dim: 1536 },
      } });
    }).then(function () {
      onboardingEl.hidden = true;
      refreshAppInfo();
    }).catch(function (err) {
      obStatus.textContent = 'failed: ' + errMsg(err);
    });
  });

  function fillVoiceSettings(v) {
    voiceVadT.value = v.vad ? v.vad.threshold : 0.5;
    voiceSilence.value = v.vad ? v.vad.silence_ms : 700;
    voiceTtsVoice.value = v.tts ? v.tts.voice : '0';
    voiceTtsSpeed.value = v.tts ? v.tts.speed : 1.0;
    invoke('get_audio_devices', {}).then(function (devs) {
      voiceMic.innerHTML = '';
      var def = document.createElement('option');
      def.value = '';
      def.textContent = '(system default)';
      voiceMic.appendChild(def);
      (devs.inputs || []).forEach(function (d) {
        var opt = document.createElement('option');
        opt.value = d.name;
        opt.textContent = d.name + (d.is_default ? ' (default)' : '');
        if (v.mic && d.name === v.mic) opt.selected = true;
        voiceMic.appendChild(opt);
      });
      if (v.mic) {
        var exists = Array.prototype.some.call(voiceMic.options, function (o) { return o.value === v.mic; });
        if (!exists) {
          var opt = document.createElement('option');
          opt.value = v.mic;
          opt.textContent = v.mic + ' (unplugged?)';
          opt.selected = true;
          voiceMic.appendChild(opt);
        }
      }
    }).catch(function () { /* mic list optional */ });
  }

  function refreshModelStatus() {
    invoke('voice_models_status', {}).then(function (st) {
      if (st.ready) {
        modelStatus.textContent = 'models ready (offline)';
        modelProgress.style.width = '100%';
      } else {
        modelStatus.textContent = 'missing: ' + (st.missing || []).join(', ');
        modelProgress.style.width = '0';
      }
      setModelDownloadActive(false);
    }).catch(function (err) {
      modelStatus.textContent = 'status failed: ' + errMsg(err);
    });
  }

  voiceModelsBtn.addEventListener('click', refreshModelStatus);

  voiceDlBtn.addEventListener('click', function () {
    setModelDownloadActive(true);
    modelStatus.textContent = 'starting download...';
    modelProgress.style.width = '0';
    invoke('voice_download_models', {}).then(function (r) {
      if (r !== 'downloading') modelStatus.textContent = r;
    }).catch(function (err) {
      modelStatus.textContent = 'failed: ' + errMsg(err);
      modelProgress.style.width = '0';
      setModelDownloadActive(false);
    });
  });

  function voicePrefsForSave() {
    var base = currentVoicePrefs || {};
    var vad = base.vad || {};
    var tts = base.tts || {};
    var stt = base.stt || {};
    var rt = base.runtime || {};
    return {
      autoplay: prefAutoplay.checked,
      enabled: base.enabled !== false,
      mic: voiceMic.value || '',
      stt: {
        engine: stt.engine || 'sherpa-onnx',
        model: stt.model || 'zipformer-en-20M-int8',
        language: stt.language || 'en',
        sample_rate: stt.sample_rate || 16000,
      },
      vad: {
        threshold: parseFloat(voiceVadT.value) || 0.5,
        silence_ms: parseInt(voiceSilence.value, 10) || 700,
        min_speech_ms: vad.min_speech_ms || 250,
      },
      tts: {
        engine: tts.engine || 'kokoro',
        voice: voiceTtsVoice.value || '0',
        speed: parseFloat(voiceTtsSpeed.value) || 1.0,
      },
      runtime: {
        max_utterance_ms: rt.max_utterance_ms || 30000,
        decode_every_frames: rt.decode_every_frames || 16,
        chunk_max_chars: rt.chunk_max_chars || 220,
        chunk_min_merge: (rt.chunk_min_merge == null ? 12 : rt.chunk_min_merge),
        num_threads: rt.num_threads || 2,
      },
    };
  }

  function loadPrefsIntoSettings() {
    invoke('get_prefs', {}).then(function (prefs) {
      prefs = prefs || {};
      prefStatus.textContent = '';
      var coding = prefs.coding || { agents: [], default: '' };
      loadAgents(coding.agents, coding.default);
      var bp = prefs.browser || {};
      browserAutoShow = bp.auto_show !== false;
      if (prefAutoshow) prefAutoshow.checked = browserAutoShow;
      applyBrowserWidth(bp.width_pct || 45, false);
      prefAutoplay.checked = !(prefs.voice && prefs.voice.autoplay === false);
      currentVoicePrefs = prefs.voice || null;
      fillVoiceSettings(prefs.voice || {});
      refreshModelStatus();
      prefFile.textContent = 'Stored in comrade.conf inside the comrade-agent home folder. The browser needs no setup — it is bundled and lives only inside this app.';
    }).catch(function (err) {
      prefStatus.textContent = 'failed: ' + errMsg(err);
    });
  }

  /* --- coding agents checklist (comrade.conf [coding]) --- */
  var agentList = document.getElementById('agent-list');
  var obDefault = document.getElementById('ob-default');
  var codeAgentList = document.getElementById('code-agent-list');
  var codeDefault = document.getElementById('code-default');
  var detectedAgents = [];

  function agentSub(a) {
    var parts = [];
    if (a.version) parts.push(a.version.split(' ').slice(0, 4).join(' '));
    if (!a.exec_supported) parts.push('manual use only');
    return parts.join(' \u00b7 ') || a.id;
  }

  function fillAgentControls(agents, enabledIds, defaultId) {
    detectedAgents = agents || [];
    var enableAll = !enabledIds || !enabledIds.length;
    agentList.innerHTML = '';
    codeAgentList.innerHTML = '';
    obDefault.innerHTML = '';
    codeDefault.innerHTML = '';
    detectedAgents.forEach(function (a) {
      var checked = enableAll || enabledIds.indexOf(a.id) >= 0;
      [[agentList, 'ob-agent'], [codeAgentList, 'set-agent']].forEach(function (pair) {
        var list = pair[0];
        var name = pair[1];
        var label = document.createElement('label');
        var box = document.createElement('input');
        box.type = 'checkbox';
        box.value = a.id;
        box.checked = checked;
        box.addEventListener('change', syncDefaultOptions);
        var span = document.createElement('span');
        span.textContent = a.name;
        var ver = document.createElement('span');
        ver.className = 'ver' + (a.exec_supported ? '' : ' noexec');
        ver.textContent = agentSub(a);
        label.appendChild(box);
        label.appendChild(span);
        label.appendChild(ver);
        list.appendChild(label);
      });
    });
    if (!detectedAgents.length) {
      agentList.innerHTML = '<p class="muted">No coding agents found. Install one, then reopen Comrade.</p>';
      codeAgentList.innerHTML = '<p class="muted">None detected.</p>';
    }
    syncDefaultOptions(defaultId);
  }

  function checkedAgentIds(listEl) {
    return Array.prototype.map.call(
      listEl.querySelectorAll('input[type="checkbox"]:checked'),
      function (b) { return b.value; }
    );
  }

  function syncDefaultOptions(keepId) {
    var execChecked = function (listEl) {
      return Array.prototype.filter.call(
        listEl.querySelectorAll('input[type="checkbox"]:checked'),
        function (b) {
          var a = detectedAgents.filter(function (x) { return x.id === b.value; })[0];
          return a && a.exec_supported;
        }
      );
    };
    [[obDefault, agentList], [codeDefault, codeAgentList]].forEach(function (pair) {
      var sel = pair[0];
      var listEl = pair[1];
      var prev = (typeof keepId === 'string' && keepId) || sel.value;
      sel.innerHTML = '';
      execChecked(listEl).forEach(function (b) {
        var a = detectedAgents.filter(function (x) { return x.id === b.value; })[0];
        var opt = document.createElement('option');
        opt.value = a.id;
        opt.textContent = a.name;
        if (a.id === prev) opt.selected = true;
        sel.appendChild(opt);
      });
    });
  }

  function loadAgents(enabledIds, defaultId) {
    return invoke('system_coding_agents', {}).then(function (agents) {
      fillAgentControls(agents, enabledIds, defaultId);
    }).catch(function (err) {
      obStatus.textContent = 'agent scan failed: ' + errMsg(err);
    });
  }

  prefSave.addEventListener('click', function () {
    var agents = checkedAgentIds(codeAgentList);
    if (!agents.length) { prefStatus.textContent = 'Enable at least one coding agent.'; return; }
    prefStatus.textContent = 'saving...';
    invoke('get_prefs', {}).then(function (existing) {
      existing = existing || {};
      return invoke('save_prefs', { prefs: {
        browser: {
          auto_show: prefAutoshow ? prefAutoshow.checked : true,
          width_pct: clampWidth(prefBwidth ? prefBwidth.value : 45),
        },
        voice: voicePrefsForSave(),
        coding: { agents: agents, default: codeDefault.value || agents[0] },
        llm: (existing && existing.llm) || { provider: 'deepseek', model: 'deepseek-flash' },
        agent: (existing && existing.agent) || { max_steps: 15, timeout_ms: 120000 },
        memory: (existing && existing.memory) || { embedding_model: 'openai/text-embedding-3-small', embedding_dim: 1536 },
      } });
    }).then(function () {
      prefStatus.textContent = 'saved to comrade.conf.';
      refreshAppInfo();
    }).catch(function (err) {
      prefStatus.textContent = 'failed: ' + errMsg(err);
    });
  });

  var _openSettings = openSettings;
  openSettings = function () {
    _openSettings();
    loadPrefsIntoSettings();
  };

  // boot: sidebar open, list chats, restore last session
  toggleSidebar(true);
  try {
    if (window.localStorage.getItem('comrade-browser-open') === '1') setBrowserOpen(true, false);
  } catch (e) { /* noop */ }
  invoke('get_prefs', {}).then(function (prefs) {
    var bp = (prefs && prefs.browser) || {};
    browserAutoShow = bp.auto_show !== false;
    if (bp.width_pct) applyBrowserWidth(bp.width_pct, false);
  }).catch(function () { /* pane keeps local defaults */ });
  invoke('app_info', {}).then(function (info) {
    if (!info.onboarded) {
      onboardingEl.hidden = false;
      loadAgents([], '');
    }
  }).catch(function () { /* settings will surface backend errors */ });
  refreshChatList();
  if (currentSessionId) {
    (function (id) {
      invoke('history_get', { sessionId: id }).then(function (msgs) {
        if (msgs && msgs.length) loadSession(id);
        else setSession(null);
      }).catch(function () { setSession(null); });
    })(currentSessionId);
  }
  memSearch.addEventListener('input', function () {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(refreshMemList, 400);
  });

  setState('idle');
})();
