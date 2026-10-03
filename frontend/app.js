import { createBrowserStream } from './browser-stream';
import { createBrowserInputQueue } from './browser-input';

/* Tauri IPC controller. React owns the shell; this controller owns the dynamic
   transcript, task steps, history, and backend-driven preference fields.
   Initialize once after the React shell has mounted. */
export function initializeComrade() {
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
  var modelPick = document.getElementById('model-pick');
  var SRV_CHAT_MODELS = [
    'nvidia/nemotron-3.5-lightning',
    'deepseek-flash',
    'openai/gpt-oss-120b',
    'google/gemma-4-31b-it',
  ];

  // Per-message model: remembered across reloads, sent verbatim each message.
  try {
    var savedModel = window.localStorage.getItem('comrade-model');
    if (savedModel && SRV_CHAT_MODELS.indexOf(savedModel) >= 0 && modelPick) modelPick.value = savedModel;
  } catch (e) { /* noop */ }
  if (modelPick) modelPick.addEventListener('change', function () {
    try { window.localStorage.setItem('comrade-model', modelPick.value); } catch (e) { /* noop */ }
  });
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
    orb.className = 'hero-mark orb ' + s;
  }

  function syncConvClass() {
    document.body.classList.toggle('conv-on', convMode);
  }

  function addMessage(role, text) {
    document.body.classList.add('has-messages');
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

  input.addEventListener('keydown', function (ev) {
    if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      form.requestSubmit();
    }
  });
  document.addEventListener('keydown', function (ev) {
    if ((ev.metaKey || ev.ctrlKey) && ev.key.toLowerCase() === 'n' && settingsEl.hidden && onboardingEl.hidden && permModal.hidden) {
      ev.preventDefault(); newChat();
    }
  });

  form.addEventListener('submit', function (ev) {
    ev.preventDefault();
    var text = input.value.trim();
    if (!text || messageStarting) return;
    messageStarting = true;
    if (input.value.trim() === text) input.value = '';
    addMessage('user', text);
    resetTaskPanel(text);
    currentResponseEl = null;
    invoke('send_message', { text: text, sessionId: currentSessionId, model: modelPick ? modelPick.value : null }).then(function (res) {
      messageStarting = false;
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
      else if (ev.status === 'done' && ev.result && !currentResponseEl) {
        // No tokens streamed (e.g. non-streaming fallback): render the result.
        currentResponseEl = addMessage('comrade', ev.result);
      }
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
    document.getElementById('hint').textContent = 'Browser preview · Launch the desktop app to chat, use voice, and access your local memory.';
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
  // Service backend controls (Settings → Service).
  var srvEnabled = document.getElementById('srv-enabled');
  var srvUrl = document.getElementById('srv-url');
  var srvKey = document.getElementById('srv-key');
  var srvLlm = document.getElementById('srv-llm');
  var srvEmb = document.getElementById('srv-emb');
  var srvDim = document.getElementById('srv-dim');
  var srvStt = document.getElementById('srv-stt');
  var srvTts = document.getElementById('srv-tts');
  var srvTest = document.getElementById('srv-test');
  var srvStatus = document.getElementById('srv-status');
  var voiceBackend = document.getElementById('voice-backend');
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
    setSettingsOpen(true);
    refreshAppInfo();
    refreshMemList();
  }

  function setSettingsOpen(open) {
    if (window.dispatchEvent) window.dispatchEvent(new CustomEvent('comrade:settings', { detail: open }));
    else settingsEl.hidden = !open;
  }

  settingsBtn.addEventListener('click', function () {
    if (settingsEl.hidden) openSettings();
    else setSettingsOpen(false);
  });


  function refreshAppInfo() {
    invoke('app_info', {}).then(function (info) {
      var kinds = (info.memory_kinds || []).map(function (k) { return k[0] + ':' + k[1]; }).join(' ');
      var backend = info.backend === 'service' ? 'service' : 'direct';
      var line = backend + ' · ' + info.provider + ' · ' + info.model +
        '  |  voice: ' + (info.voice_backend || 'local');
      if (info.backend === 'service' && info.server_url) line += ' (' + info.server_url + ')';
      line += '  |  memories: ' + info.memory_count + (kinds ? ' (' + kinds + ')' : '');
      modelInfo.textContent = line;
      // Composer picker follows the active brain model unless the user picked one.
      try {
        if (modelPick && !window.localStorage.getItem('comrade-model') &&
            info.model && SRV_CHAT_MODELS.indexOf(info.model) >= 0) {
          modelPick.value = info.model;
        }
      } catch (e) { /* noop */ }
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
    greeting.textContent = 'What’s on your mind?';
    subtitle.textContent = 'Think it through. Build it out. Make it happen.';
    document.body.classList.remove('has-messages');
    refreshChatList();
    if (browserPrimary) revealChat(true, true);
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
  var browserKeys = document.getElementById('browser-keys');
  var browserIsOpen = false;
  var browserPrimary = false;
  var chatPinned = false;
  var chatRevealed = false;
  var chatLatched = false;
  var chatHideTimer = null;
  var windowFullscreen = false;
  var fullscreenBusy = false;
  var chatWorkspace = document.getElementById('chat-workspace');
  var chatRevealBtn = document.getElementById('chat-reveal');
  var chatPinBtn = document.getElementById('chat-pin');
  var chatCloseBtn = document.getElementById('chat-close');
  var browserExpandBtn = document.getElementById('browser-expand');
  var browserFullscreenBtn = document.getElementById('browser-fullscreen');
  var browserTabs = document.getElementById('browser-tabs');
  var browserNewTabBtn = document.getElementById('browser-new-tab');
  var browserHomeBtn = document.getElementById('browser-home');
  var browserMenu = document.getElementById('browser-menu');
  var browserCopyBtn = document.getElementById('browser-copy');
  var DUCKDUCKGO_HOME = 'https://duckduckgo.com/';
  var addressShortcut = document.querySelector('.address-shortcut');
  if (addressShortcut && /Mac|iPhone|iPad/.test(navigator.platform)) addressShortcut.textContent = '⌘ L';

  function renderBrowserLayout() {
    document.body.classList.toggle('browser-primary', browserPrimary);
    document.body.classList.toggle('chat-pinned', browserPrimary && chatPinned);
    document.body.classList.toggle('chat-revealed', browserPrimary && chatRevealed);
    chatWorkspace.inert = browserPrimary && !chatPinned && !chatRevealed;
    browserDivider.hidden = !browserIsOpen || browserPrimary;
    browserExpandBtn.setAttribute('aria-pressed', String(browserPrimary));
    browserExpandBtn.setAttribute('aria-label', browserPrimary ? 'Return to chat view' : 'Make browser the main view');
    browserExpandBtn.title = browserPrimary ? 'Return to chat view' : 'Make browser the main view';
    chatRevealBtn.setAttribute('aria-expanded', String(chatPinned || chatRevealed));
    chatPinBtn.setAttribute('aria-pressed', String(chatPinned));
    chatPinBtn.setAttribute('aria-label', chatPinned ? 'Float chat over browser' : 'Pin chat beside browser');
    chatPinBtn.title = chatPinned ? 'Float chat over browser' : 'Pin chat beside browser';
  }
  function revealChat(open, latch) {
    clearTimeout(chatHideTimer);
    chatRevealed = !!open;
    chatLatched = !!open && !!latch;
    renderBrowserLayout();
  }
  function scheduleChatHide() {
    clearTimeout(chatHideTimer);
    chatHideTimer = setTimeout(function () {
      if (!chatPinned && !chatLatched && !chatWorkspace.contains(document.activeElement)) revealChat(false);
    }, 220);
  }
  function setBrowserPrimary(primary) {
    browserPrimary = !!primary;
    if (browserPrimary && !browserIsOpen) setBrowserOpen(true);
    chatRevealed = false;
    chatLatched = false;
    clearTimeout(chatHideTimer);
    renderBrowserLayout();
    try { window.localStorage.setItem('comrade-browser-primary', browserPrimary ? '1' : '0'); } catch (e) {}
    if (browserPrimary) browserExpandBtn.focus();
  }
  async function setWindowFullscreen(fullscreen) {
    if (fullscreenBusy) return;
    fullscreenBusy = true;
    try {
      var desktopWindow = window.__TAURI__ && window.__TAURI__.window;
      if (desktopWindow) await desktopWindow.getCurrentWindow().setFullscreen(fullscreen);
      else if (fullscreen) await document.documentElement.requestFullscreen();
      else if (document.fullscreenElement) await document.exitFullscreen();
      windowFullscreen = fullscreen;
      browserFullscreenBtn.setAttribute('aria-pressed', String(fullscreen));
      browserFullscreenBtn.setAttribute('aria-label', fullscreen ? 'Exit fullscreen' : 'Enter fullscreen');
      browserFullscreenBtn.title = fullscreen ? 'Exit fullscreen (F11)' : 'Fullscreen (F11)';
    } catch (error) {
      browserStatus.textContent = 'Could not change fullscreen: ' + error.message;
    } finally { fullscreenBusy = false; }
  }
  browserExpandBtn.addEventListener('click', function () { setBrowserPrimary(!browserPrimary); });
  browserFullscreenBtn.addEventListener('click', function () {
    if (!windowFullscreen) setBrowserPrimary(true);
    setWindowFullscreen(!windowFullscreen);
  });
  chatRevealBtn.addEventListener('mouseenter', function () { if (!chatPinned && !chatLatched) revealChat(true); });
  chatRevealBtn.addEventListener('mouseleave', scheduleChatHide);
  chatRevealBtn.addEventListener('focus', function () { if (browserPrimary) revealChat(true); });
  chatRevealBtn.addEventListener('click', function () {
    if (chatPinned) { chatPinned = false; revealChat(false); }
    else revealChat(!chatLatched, !chatLatched);
    persistChatLayout();
  });
  chatWorkspace.addEventListener('mouseenter', function () { clearTimeout(chatHideTimer); });
  chatWorkspace.addEventListener('mouseleave', scheduleChatHide);
  chatWorkspace.addEventListener('focusout', scheduleChatHide);
  function persistChatLayout() {
    try { window.localStorage.setItem('comrade-chat-pinned', chatPinned ? '1' : '0'); } catch (e) {}
  }
  chatPinBtn.addEventListener('click', function () {
    chatPinned = !chatPinned;
    revealChat(true, true);
    persistChatLayout();
  });
  chatCloseBtn.addEventListener('click', function () {
    chatPinned = false;
    revealChat(false);
    persistChatLayout();
    browserExpandBtn.focus();
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'F11' && browserIsOpen) {
      event.preventDefault();
      if (!windowFullscreen) setBrowserPrimary(true);
      setWindowFullscreen(!windowFullscreen);
    } else if (event.key === 'Escape' && browserPrimary && document.getElementById('settings').hidden && document.getElementById('perm-modal').hidden) {
      if ((chatRevealed || chatPinned) && !windowFullscreen) {
        chatPinned = false;
        revealChat(false);
        persistChatLayout();
        browserExpandBtn.focus();
      } else {
        setWindowFullscreen(false);
        setBrowserPrimary(false);
      }
    }
  });
  document.addEventListener('fullscreenchange', function () {
    if (!document.fullscreenElement && !(window.__TAURI__ && window.__TAURI__.window)) {
      windowFullscreen = false;
      browserFullscreenBtn.setAttribute('aria-pressed', 'false');
      browserFullscreenBtn.setAttribute('aria-label', 'Enter fullscreen');
      browserFullscreenBtn.title = 'Fullscreen (F11)';
    }
  });

  var browserMetadataTimer = null;
  var browserReconnectTimer = null;
  var queueBrowserInput = createBrowserInputQueue(invoke);
  function paneDimensions() {
    return { width: Math.max(240, Math.round(browserView.clientWidth || 640)), height: Math.max(120, Math.round(browserView.clientHeight || 640)) };
  }
  var browserStream = createBrowserStream({
    invoke: invoke, tauri: tauri, image: browserImg,
    onFrame: function (frame) {
      browserPageW = frame.width; browserPageH = frame.height;
      if (browserView.classList.contains('idle')) browserView.classList.remove('idle');
      if (browserStatus.textContent) browserStatus.textContent = '';
    },
    onError: function (error) {
      browserStatus.textContent = 'Reconnecting browser…';
      if (browserReconnectTimer) clearTimeout(browserReconnectTimer);
      if (browserIsOpen && !document.hidden) browserReconnectTimer = setTimeout(function () { ensureBrowserReady(); }, 1000);
    },
  });
  var browserAutoShow = true;
  var browserRefreshing = false;
  // Last known page size (CSS px) for mapping image clicks onto the page.
  var browserPageW = 0;
  var browserPageH = 0;
  var browserInteractTimer = null;
  var browserWheelDebt = { x: 0, y: 0 };
  var browserWheelTimer = null;
  var browserTabsSignature = '';
  var browserTabsRequest = null;
  var activeBrowserTabId = null;
  var activeBrowserTabUrl = '';
  var browserTabBusy = false;
  var browserTabOrder = [];
  var browserTabElements = new Map();
  var browserTabRevision = 0;
  var browserTabsRequestVersion = 0;
  var browserRequestedTabId = null;

  function tabLabel(tab) {
    if (tab.title) return tab.title;
    if (!tab.url || tab.url === 'about:blank') return 'New tab';
    try { return new URL(tab.url).hostname.replace(/^www\./, '') || 'New tab'; } catch (e) { return 'New tab'; }
  }

  function tabFaviconUrl(tab) {
    try {
      var site = new URL(tab.url);
      if (site.protocol !== 'https:' && site.protocol !== 'http:') return '';
      if (tab.favicon_url) {
        var icon = new URL(tab.favicon_url, site);
        if (icon.protocol === 'https:' || icon.protocol === 'http:' || icon.href.startsWith('data:image/')) return icon.href;
      }
      return new URL('/favicon.ico', site.origin).href;
    } catch (error) { return ''; }
  }

  function updateTabFavicon(element, tab) {
    var url = tabFaviconUrl(tab);
    if (element.dataset.iconUrl === url) return;
    element.dataset.iconUrl = url;
    element.classList.remove('has-icon');
    element.innerHTML = '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c5 5 5 13 0 18-5-5-5-13 0-18Z"/></svg>';
    if (!url) return;
    var image = document.createElement('img');
    image.alt = '';
    image.decoding = 'async';
    image.referrerPolicy = 'no-referrer';
    image.onload = function () { if (element.dataset.iconUrl === url) element.classList.add('has-icon'); };
    var fallback = new URL('/favicon.ico', tab.url).href;
    image.onerror = function () {
      if (image.getAttribute('src') !== fallback) image.src = fallback;
      else { image.remove(); element.classList.remove('has-icon'); }
    };
    image.src = url;
    element.appendChild(image);
  }

  function renderBrowserTabs(tabs) {
    if (!browserTabs) return;
    tabs = tabs || [];
    var ids = tabs.map(function (tab) { return tab.id; });
    browserTabOrder = browserTabOrder.filter(function (id) { return ids.indexOf(id) >= 0; });
    tabs.forEach(function (tab) { if (browserTabOrder.indexOf(tab.id) < 0) browserTabOrder.push(tab.id); });
    tabs = browserTabOrder.map(function (id) { return tabs.find(function (tab) { return tab.id === id; }); });
    var signature = JSON.stringify(tabs);
    if (signature === browserTabsSignature) return;
    browserTabsSignature = signature;
    var active = tabs.find(function (tab) { return tab.active; });
    var previousActiveId = activeBrowserTabId;
    activeBrowserTabId = active?.id || null;
    activeBrowserTabUrl = active?.url || '';
    browserTabElements.forEach(function (item, id) {
      if (ids.indexOf(id) < 0) { item.remove(); browserTabElements.delete(id); }
    });
    tabs.forEach(function (tab, index) {
      var item = browserTabElements.get(tab.id);
      if (!item) {
        item = document.createElement('div');
        item.setAttribute('role', 'tab');
        item.dataset.tabId = tab.id;
        var favicon = document.createElement('span');
        favicon.className = 'browser-tab-favicon';
        var label = document.createElement('span');
        label.className = 'browser-tab-label';
        var close = document.createElement('button');
        close.className = 'browser-tab-close';
        close.type = 'button';
        close.innerHTML = '&times;';
        close.addEventListener('click', function (event) {
          event.stopPropagation();
          closeBrowserTab(tab.id);
        });
        item.append(favicon, label, close);
        item.addEventListener('click', function () {
          if (!item.browserTab.active || browserTabBusy) selectBrowserTab(tab.id);
        });
        item.addEventListener('keydown', function (event) {
          if (event.target !== item) return;
          if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); selectBrowserTab(tab.id); }
          if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].indexOf(event.key) >= 0) {
            event.preventDefault();
            var items = Array.from(browserTabs.querySelectorAll('[role="tab"]'));
            var current = items.indexOf(item);
            var next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
              : (current + (event.key === 'ArrowRight' ? 1 : -1) + items.length) % items.length;
            items[next].focus();
          }
        });
        browserTabElements.set(tab.id, item);
      }
      item.browserTab = tab;
      item.className = 'browser-tab' + (tab.active ? ' active' : '');
      item.setAttribute('aria-selected', String(!!tab.active));
      item.setAttribute('tabindex', tab.active ? '0' : '-1');
      item.title = tabLabel(tab);
      var favicon = item.children[0];
      updateTabFavicon(favicon, tab);
      item.children[1].textContent = tabLabel(tab);
      item.children[2].setAttribute('aria-label', 'Close ' + tabLabel(tab));
      if (browserTabs.children[index] !== item) browserTabs.insertBefore(item, browserTabs.children[index] || null);
    });
    if (activeBrowserTabId !== previousActiveId) {
      var selected = browserTabElements.get(activeBrowserTabId);
      if (selected) selected.scrollIntoView({ inline: 'nearest', block: 'nearest' });
    }
  }

  function refreshBrowserTabs() {
    var version = browserTabRevision;
    if (browserTabsRequest) {
      return browserTabsRequestVersion === version ? browserTabsRequest : browserTabsRequest.then(refreshBrowserTabs);
    }
    browserTabsRequestVersion = version;
    browserTabsRequest = invoke('browser_tabs', {}).then(function (tabs) {
      if (version === browserTabRevision) renderBrowserTabs(tabs);
    }).catch(function () { /* keep last stable strip */ })
      .finally(function () { browserTabsRequest = null; });
    return browserTabsRequest;
  }

  function finishBrowserTabOperation() {
    browserTabBusy = false;
    var requested = browserRequestedTabId;
    browserRequestedTabId = null;
    if (requested && requested !== activeBrowserTabId && browserTabElements.has(requested)) selectBrowserTab(requested);
    else refreshBrowser();
  }

  function restartBrowserStream() {
    browserStream.stop();
    if (!browserIsOpen || document.hidden) return Promise.resolve();
    var size = paneDimensions();
    return browserStream.start(size.width, size.height).then(function () { refreshBrowser(); });
  }

  function selectBrowserTab(id) {
    if (browserTabBusy) { browserRequestedTabId = id; return; }
    if (id === activeBrowserTabId) return;
    browserTabBusy = true;
    browserTabRevision++;
    queueBrowserInput.clear();
    if (browserStatus) browserStatus.textContent = 'Switching tab…';
    invoke('browser_tab_select', { id: id }).then(function (tab) {
      if (tab.url) browserUrl.value = tab.url;
      if (tab.title) browserTitle.textContent = tab.title;
      return restartBrowserStream();
    }).then(refreshBrowserTabs).catch(function (err) {
      if (browserStatus) browserStatus.textContent = 'Could not switch tabs: ' + errMsg(err);
    }).finally(finishBrowserTabOperation);
  }

  function createBrowserTab() {
    if (browserTabBusy) return;
    browserTabBusy = true;
    browserTabRevision++;
    queueBrowserInput.clear();
    if (browserStatus) browserStatus.textContent = 'Opening a new tab…';
    invoke('browser_tab_new', { url: DUCKDUCKGO_HOME }).then(function (tab) {
      browserUrl.value = tab.url || DUCKDUCKGO_HOME;
      return restartBrowserStream();
    }).then(function () { browserUrl.focus(); browserUrl.select(); return refreshBrowserTabs(); }).catch(function (err) {
      if (browserStatus) browserStatus.textContent = 'Could not open a tab: ' + errMsg(err);
    }).finally(finishBrowserTabOperation);
  }

  function closeBrowserTab(id) {
    if (browserTabBusy) return;
    browserTabBusy = true;
    browserTabRevision++;
    var closesActiveTab = id === activeBrowserTabId;
    if (closesActiveTab) queueBrowserInput.clear();
    invoke('browser_tab_close', { id: id }).then(function (tab) {
      if (tab.url) browserUrl.value = tab.url;
      return closesActiveTab ? restartBrowserStream() : undefined;
    }).then(refreshBrowserTabs).catch(function (err) {
      if (browserStatus) browserStatus.textContent = 'Could not close the tab: ' + errMsg(err);
    }).finally(finishBrowserTabOperation);
  }

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
    if (!browserIsOpen && browserPrimary) { setBrowserPrimary(false); if (windowFullscreen) setWindowFullscreen(false); }
    renderBrowserLayout();
    try { window.localStorage.setItem('comrade-browser-open', browserIsOpen ? '1' : '0'); } catch (e) { /* noop */ }
    if (browserMetadataTimer) { clearInterval(browserMetadataTimer); browserMetadataTimer = null; }
    if (browserReconnectTimer) { clearTimeout(browserReconnectTimer); browserReconnectTimer = null; }
    if (!browserIsOpen) { queueBrowserInput.clear(); browserStream.stop(); stopInstallPoll(); }
    if (browserIsOpen && refresh !== false && !document.hidden) ensureBrowserReady();
    if (browserIsOpen && browserStream.supported) {
      browserMetadataTimer = setInterval(function () { if (!document.hidden && browserStream.active) refreshBrowser(); }, 1000);
    }
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
    if (browserTabBusy) return Promise.resolve(false);
    var version = browserTabRevision;
    if (browserStatus) browserStatus.textContent = 'Starting built-in browser…';
    return invoke('browser_ensure', {}).then(function (st) {
      if (version !== browserTabRevision || browserTabBusy) return false;
      stopInstallPoll();
      st = st || {};
      if (st.url && st.url !== 'about:blank' && browserUrl) browserUrl.value = st.url;
      if (st.title && browserTitle) browserTitle.textContent = st.title;
      if (!st.url || st.url === 'about:blank') {
        return invoke('browser_open', { url: DUCKDUCKGO_HOME }).then(function (home) {
          if (home && home.url) browserUrl.value = home.url;
          return ensureBrowserReady();
        });
      }
      if (Array.isArray(st.tabs)) renderBrowserTabs(st.tabs);
      else refreshBrowserTabs();
      if (!browserIsOpen || document.hidden) return false;
      if (browserStream.supported) {
        var size = paneDimensions();
        return browserStream.start(size.width, size.height).then(function (ok) { refreshBrowser(); return ok; });
      }
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
    if (browserRefreshing || browserTabBusy) return;
    browserRefreshing = true;
    var version = browserTabRevision;
    if (!browserStream.active && browserStatus) browserStatus.textContent = 'updating…';
    invoke('browser_state', {}).then(function (st) {
      if (version !== browserTabRevision || browserTabBusy || !browserIsOpen) return null;
      st = st || {};
      if (st.url && browserUrl && document.activeElement !== browserUrl) browserUrl.value = st.url;
      if (browserTitle) browserTitle.textContent = st.title || st.url || 'Comrade browser';
      if (!st.running) {
        if (browserStatus) browserStatus.textContent = 'Browser is idle. Open a page or run a task.';
        if (browserView) browserView.classList.add('idle');
        return null;
      }
      if (browserTabBusy) return null;
      if (Array.isArray(st.tabs)) renderBrowserTabs(st.tabs);
      else refreshBrowserTabs();
      if (browserView) browserView.classList.remove('idle');
      return browserStream.active ? null : invoke('browser_frame', {});
    }).then(function (frame) {
      if (version !== browserTabRevision || browserTabBusy) return;
      if (frame && frame.data_url && browserImg) browserImg.src = frame.data_url;
      if (frame && frame.width > 0) { browserPageW = frame.width; browserPageH = frame.height; }
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
  if (browserShotBtn) browserShotBtn.addEventListener('click', function () {
    if (browserMenu) browserMenu.open = false;
    refreshBrowser();
  });
  if (browserCopyBtn) browserCopyBtn.addEventListener('click', function () {
    browserMenu.open = false;
    var url = activeBrowserTabUrl || browserUrl.value.trim();
    if (!url) return;
    Promise.resolve().then(function () { return navigator.clipboard.writeText(url); }).then(function () {
      browserStatus.textContent = 'Link copied';
    }).catch(function () {
      browserUrl.focus(); browserUrl.select();
      browserStatus.textContent = 'Press Ctrl+C / ⌘C to copy the selected link';
    });
  });
  document.addEventListener('click', function (event) {
    if (browserMenu && browserMenu.open && !browserMenu.contains(event.target)) browserMenu.open = false;
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape' && browserMenu && browserMenu.open) {
      event.preventDefault(); event.stopImmediatePropagation();
      browserMenu.open = false;
      browserMenu.querySelector('summary').focus();
    }
  }, true);
  if (browserNewTabBtn) browserNewTabBtn.addEventListener('click', createBrowserTab);
  if (browserHomeBtn) browserHomeBtn.addEventListener('click', function () {
    browserUrl.value = DUCKDUCKGO_HOME;
    browserForm.requestSubmit();
  });
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

  document.addEventListener('keydown', function (event) {
    if (!browserIsOpen || document.getElementById('settings').hidden === false) return;
    var modifier = event.ctrlKey || event.metaKey;
    if (modifier && event.key.toLowerCase() === 'l') {
      event.preventDefault(); event.stopPropagation(); browserUrl.focus(); browserUrl.select();
    } else if (modifier && event.key.toLowerCase() === 't') {
      event.preventDefault(); event.stopPropagation(); createBrowserTab();
    } else if (modifier && event.key.toLowerCase() === 'w') {
      var active = browserTabs && browserTabs.querySelector('.browser-tab.active');
      var close = active && active.querySelector('.browser-tab-close');
      if (close) { event.preventDefault(); event.stopPropagation(); close.click(); }
    } else if (browserPane.contains(document.activeElement) &&
        ((modifier && event.key.toLowerCase() === 'r') || event.key === 'F5')) {
      event.preventDefault(); event.stopPropagation(); browserReloadBtn.click();
    } else if (browserPane.contains(document.activeElement) && event.altKey &&
        (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
      event.preventDefault(); event.stopPropagation();
      (event.key === 'ArrowLeft' ? browserBackBtn : browserFwdBtn).click();
    }
  }, true);

  // --- Direct interaction: the view is live. Clicks, typing, and wheel
  // events on the image drive the same bundled Chromium the agent uses. ---
  function interactRefreshSoon() {
    if (browserStream.active) return; // Chromium pushes visual changes without another RPC.
    if (browserInteractTimer) return;
    browserInteractTimer = setTimeout(function () { browserInteractTimer = null; refreshBrowser(); }, 16);
  }

  function imageToPage(ev) {
    if (!browserImg || !browserPageW || !browserImg.clientWidth) return null;
    var r = browserImg.getBoundingClientRect ? browserImg.getBoundingClientRect() : null;
    var dx = (r && ev.clientX != null) ? ev.clientX - r.left : (ev.offsetX || 0);
    var dy = (r && ev.clientY != null) ? ev.clientY - r.top : (ev.offsetY || 0);
    var scale = browserPageW / browserImg.clientWidth;
    return { x: Math.round(dx * scale), y: Math.round(dy * scale) };
  }

  if (browserImg) browserImg.addEventListener('click', function (ev) {
    if (browserStream.active) return;
    var pt = imageToPage(ev);
    if (!pt) return;
    if (browserStatus) browserStatus.textContent = 'clicking…';
    queueBrowserInput('browser_click_at', { x: pt.x, y: pt.y }).then(function (r) {
      if (r && r.url && browserUrl) browserUrl.value = r.url;
      // Route subsequent keystrokes into the page (e.g. a focused field).
      if (browserKeys) browserKeys.focus();
      interactRefreshSoon();
    }).catch(function (err) { if (browserStatus) browserStatus.textContent = 'Click failed: ' + errMsg(err); });
  });

  var pointerMove = null, pointerMoveTimer = null;
  function pointerPayload(ev, kind) {
    var point = imageToPage(ev);
    if (!point) return null;
    return { type: kind, x: Math.max(0, Math.min(browserPageW - 1, point.x)), y: Math.max(0, Math.min(browserPageH - 1, point.y)),
      button: ['left', 'middle', 'right'][ev.button] || 'none', buttons: ev.buttons || 0,
      modifiers: (ev.altKey ? 1 : 0) | (ev.ctrlKey ? 2 : 0) | (ev.metaKey ? 4 : 0) | (ev.shiftKey ? 8 : 0),
      clickCount: kind === 'mouseMoved' ? 0 : (ev.detail || 1) };
  }
  function flushPointerMove() {
    if (pointerMoveTimer) { clearTimeout(pointerMoveTimer); pointerMoveTimer = null; }
    if (browserTabBusy || !browserIsOpen) { pointerMove = null; return; }
    if (pointerMove) { var event = pointerMove; pointerMove = null; queueBrowserInput('browser_pointer', { event: event }).catch(function () {}); }
  }
  if (browserImg) {
    browserImg.addEventListener('pointerdown', function (ev) {
      if (!browserStream.active || browserTabBusy) return;
      var event = pointerPayload(ev, 'mousePressed'); if (!event) return;
      ev.preventDefault(); flushPointerMove();
      if (browserImg.setPointerCapture) browserImg.setPointerCapture(ev.pointerId);
      queueBrowserInput('browser_pointer', { event: event }).catch(function (err) { browserStatus.textContent = errMsg(err); });
    });
    browserImg.addEventListener('pointerup', function (ev) {
      if (!browserStream.active || browserTabBusy) return;
      var event = pointerPayload(ev, 'mouseReleased'); if (!event) return;
      ev.preventDefault(); flushPointerMove();
      queueBrowserInput('browser_pointer', { event: event }).catch(function (err) { browserStatus.textContent = errMsg(err); });
      browserKeys.focus();
    });
    browserImg.addEventListener('pointercancel', function (ev) {
      if (!browserStream.active || browserTabBusy) return;
      var event = pointerPayload(ev, 'mouseReleased');
      if (event) { event.buttons = 0; queueBrowserInput('browser_pointer', { event: event }).catch(function () {}); }
    });
    browserImg.addEventListener('pointermove', function (ev) {
      if (!browserStream.active || browserTabBusy) return;
      pointerMove = pointerPayload(ev, 'mouseMoved');
      if (!pointerMoveTimer) pointerMoveTimer = setTimeout(flushPointerMove, 16);
    });
    browserImg.addEventListener('contextmenu', function (ev) { if (browserStream.active) ev.preventDefault(); });
  }

  if (browserView) browserView.addEventListener('wheel', function (ev) {
    if (!browserIsOpen || browserTabBusy) return;
    ev.preventDefault();
    var factor = ev.deltaMode === 1 ? 16 : ev.deltaMode === 2 ? (browserPageH || 640) : 1;
    browserWheelDebt.x += (ev.deltaX || 0) * factor;
    browserWheelDebt.y += (ev.deltaY || 0) * factor;
    browserWheelDebt.modifiers = (ev.altKey ? 1 : 0) | (ev.ctrlKey ? 2 : 0) | (ev.metaKey ? 4 : 0) | (ev.shiftKey ? 8 : 0);
    browserWheelDebt.point = imageToPage(ev) || { x: browserPageW / 2, y: browserPageH / 2 };
    if (browserWheelTimer) return;
    browserWheelTimer = setTimeout(function () {
      browserWheelTimer = null;
      var dx = Math.round(browserWheelDebt.x);
      var dy = Math.round(browserWheelDebt.y);
      browserWheelDebt.x = 0; browserWheelDebt.y = 0;
      if ((!dx && !dy) || browserTabBusy || !browserIsOpen) return;
      var request = browserStream.active
        ? queueBrowserInput('browser_pointer', { event: { type: 'mouseWheel', x: browserWheelDebt.point.x, y: browserWheelDebt.point.y, deltaX: dx, deltaY: dy, modifiers: browserWheelDebt.modifiers } })
        : queueBrowserInput('browser_scroll', { x: dx, y: dy });
      request.then(function () {
        interactRefreshSoon();
      }).catch(function () { /* keep the last good frame */ });
    }, 16);
  }, { passive: false });

  var SPECIAL_KEYS = {
    Enter: 'Enter', Tab: 'Tab', Escape: 'Escape', Esc: 'Escape',
    Backspace: 'Backspace', Delete: 'Delete', ArrowLeft: 'ArrowLeft',
    ArrowUp: 'ArrowUp', ArrowRight: 'ArrowRight', ArrowDown: 'ArrowDown',
    Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown',
  };

  if (browserKeys) {
    browserKeys.addEventListener('keydown', function (ev) {
      if (browserTabBusy) { ev.preventDefault(); return; }
      if (ev.isComposing) return;
      var key = SPECIAL_KEYS[ev.key] || ((ev.ctrlKey || ev.metaKey || ev.altKey) && ev.key.length === 1 ? ev.key : null);
      if (!key) return;
      ev.preventDefault();
      queueBrowserInput('browser_press_key', { key: key, modifiers: (ev.altKey ? 1 : 0) | (ev.ctrlKey ? 2 : 0) | (ev.metaKey ? 4 : 0) | (ev.shiftKey ? 8 : 0) }).then(function () {
        interactRefreshSoon();
      }).catch(function (err) { if (browserStatus) browserStatus.textContent = 'Key failed: ' + errMsg(err); });
    });
    browserKeys.addEventListener('input', function () {
      var text = browserKeys.value;
      if (!text) return;
      browserKeys.value = '';
      if (browserTabBusy) return;
      queueBrowserInput('browser_type_text', { text: text }).then(function () {
        interactRefreshSoon();
      }).catch(function (err) { if (browserStatus) browserStatus.textContent = 'Type failed: ' + errMsg(err); });
    });
  }

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

  document.addEventListener('visibilitychange', function () {
    if (document.hidden) browserStream.stop();
    else if (browserIsOpen) ensureBrowserReady();
  });
  window.addEventListener && window.addEventListener('pagehide', function () { browserStream.stop(); });
  if (typeof ResizeObserver !== 'undefined') {
    var resizeTimer;
    new ResizeObserver(function () {
      if (resizeTimer) clearTimeout(resizeTimer);
      if (!browserIsOpen || !browserStream.active) return;
      resizeTimer = setTimeout(function () {
        var size = paneDimensions();
        browserStream.resize(size.width, size.height).catch(function () {});
      }, 80);
    }).observe(browserView);
  }

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
  var prefAdblock = document.getElementById('pref-adblock');
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
        existing.browser = Object.assign({}, existing.browser, { auto_show: auto, width_pct: width, adblock_enabled: prefAdblock ? prefAdblock.checked : true });
        return invoke('save_prefs', { prefs: existing });
      }).then(function () {
        if (prefStatus) prefStatus.textContent = 'Browser pane settings saved.';
      }).catch(function (err) {
        if (prefStatus) prefStatus.textContent = 'Browser pane not saved: ' + errMsg(err);
      });
    }, 400);
  }

  if (prefAdblock) prefAdblock.addEventListener('change', persistBrowserPrefs);
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
    browserStream.stop();
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
          autoplay: true, enabled: true, mic: v.mic || '', backend: 'server',
          stt: v.stt || { engine: 'sherpa-onnx', model: 'zipformer-en-20M-int8', language: 'en', sample_rate: 16000 },
          vad: v.vad || { threshold: 0.5, silence_ms: 700, min_speech_ms: 250 },
          tts: v.tts || { engine: 'kokoro', voice: '0', speed: 1.0 },
          runtime: v.runtime || { max_utterance_ms: 30000, decode_every_frames: 16, chunk_max_chars: 220, chunk_min_merge: 12, num_threads: 2 },
        },
        coding: { agents: agents, default: obDefault.value || agents[0] },
        llm: (existing && existing.llm) || { provider: 'deepseek', model: 'deepseek-flash' },
        agent: (existing && existing.agent) || { max_steps: 15, timeout_ms: 120000 },
        memory: (existing && existing.memory) || { embedding_model: 'openai/text-embedding-3-small', embedding_dim: 1536 },
        server: (existing && existing.server) || { enabled: false, base_url: '', api_key: '', llm_model: 'deepseek-flash', embedding_model: 'comrade-embed', embedding_dim: 1536, stt_model: 'comrade-stt', tts_voice: 'default' },
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
    if (voiceBackend) voiceBackend.value = (v.backend === 'local') ? 'local' : 'server';
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

  var SRV_LLM_MODELS = [
    'nvidia/nemotron-3.5-lightning',
    'deepseek-flash',
    'openai/gpt-oss-120b',
    'google/gemma-4-31b-it',
  ];
  var SRV_DEFAULT_LLM = 'deepseek-flash';

  function normSrvLlm(v) {
    v = (v || '').trim();
    return SRV_LLM_MODELS.indexOf(v) >= 0 ? v : SRV_DEFAULT_LLM;
  }

  function fillServerSettings(s) {
    s = s || {};
    if (srvEnabled) srvEnabled.checked = s.enabled === true;
    if (srvUrl) srvUrl.value = s.base_url || '';
    if (srvKey) srvKey.value = s.api_key || '';
    if (srvLlm) srvLlm.value = normSrvLlm(s.llm_model);
    if (srvEmb) srvEmb.value = s.embedding_model || 'comrade-embed';
    if (srvDim) srvDim.value = s.embedding_dim || 1536;
    if (srvStt) srvStt.value = s.stt_model || 'comrade-stt';
    if (srvTts) srvTts.value = s.tts_voice || 'default';
    if (srvStatus) srvStatus.textContent = '';
  }

  function serverPrefsForSave() {
    return {
      enabled: !!(srvEnabled && srvEnabled.checked),
      base_url: (srvUrl && srvUrl.value.trim()) || '',
      api_key: (srvKey && srvKey.value) || '',
      llm_model: normSrvLlm(srvLlm && srvLlm.value),
      embedding_model: (srvEmb && srvEmb.value.trim()) || 'comrade-embed',
      embedding_dim: parseInt(srvDim && srvDim.value, 10) || 1536,
      stt_model: (srvStt && srvStt.value.trim()) || 'comrade-stt',
      tts_voice: (srvTts && srvTts.value.trim()) || 'default',
    };
  }

  if (srvTest) srvTest.addEventListener('click', function () {
    if (srvStatus) srvStatus.textContent = 'probing server...';
    invoke('server_status', {}).then(function (r) {
      if (srvStatus) srvStatus.textContent = r + ' — loading models...';
      return invoke('server_models', {});
    }).then(function (m) {
      var ids = [];
      var data = (m && m.data) || [];
      data.forEach(function (entry) {
        if (entry && entry.id) ids.push(entry.id);
      });
      var shown = ids.slice(0, 12).join(', ') + (ids.length > 12 ? ' …(+' + (ids.length - 12) + ' more)' : '');
      if (srvStatus) srvStatus.textContent = (srvStatus.textContent || '').replace(' — loading models...', '') +
        (ids.length ? '  |  models: ' + shown : '  |  no models listed.');
    }).catch(function (err) {
      if (srvStatus) srvStatus.textContent = 'unreachable: ' + errMsg(err);
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
      backend: (voiceBackend && voiceBackend.value === 'server') ? 'server' : 'local',
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
      if (prefAdblock) prefAdblock.checked = bp.adblock_enabled !== false;
      applyBrowserWidth(bp.width_pct || 45, false);
      prefAutoplay.checked = !(prefs.voice && prefs.voice.autoplay === false);
      currentVoicePrefs = prefs.voice || null;
      fillVoiceSettings(prefs.voice || {});
      fillServerSettings(prefs.server || {});
      refreshModelStatus();
      prefFile.textContent = 'Preferences are stored locally on this device.';
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
          adblock_enabled: prefAdblock ? prefAdblock.checked : true,
        },
        voice: voicePrefsForSave(),
        coding: { agents: agents, default: codeDefault.value || agents[0] },
        llm: (existing && existing.llm) || { provider: 'deepseek', model: 'deepseek-flash' },
        agent: (existing && existing.agent) || { max_steps: 15, timeout_ms: 120000 },
        memory: (existing && existing.memory) || { embedding_model: 'openai/text-embedding-3-small', embedding_dim: 1536 },
        server: serverPrefsForSave(),
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
    chatPinned = window.localStorage.getItem('comrade-chat-pinned') === '1';
    if (window.localStorage.getItem('comrade-browser-open') === '1') {
      setBrowserOpen(true);
      if (window.localStorage.getItem('comrade-browser-primary') === '1') setBrowserPrimary(true);
    }
  } catch (e) { /* noop */ }
  invoke('get_prefs', {}).then(function (prefs) {
    var bp = (prefs && prefs.browser) || {};
    browserAutoShow = bp.auto_show !== false;
    if (prefAdblock) prefAdblock.checked = bp.adblock_enabled !== false;
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
}
