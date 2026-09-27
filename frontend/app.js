/* Comrade frontend (vanilla JS). Talks to the Rust backend via Tauri IPC only. */
(function () {
  'use strict';

  var tauri = (window.__TAURI__ && window.__TAURI__.core) || null;
  var tauriEvent = (window.__TAURI__ && window.__TAURI__.event) || null;

  var pill = document.getElementById('status-pill');
  var statusText = document.getElementById('status-text');
  var orb = document.getElementById('orb');
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
  var permModal = document.getElementById('perm-modal');
  var permSummary = document.getElementById('perm-summary');
  var permAllow = document.getElementById('perm-allow');
  var permCancel = document.getElementById('perm-cancel');

  var currentResponseEl = null;
  var pendingPermId = null;
  var currentAudio = null;

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

  var recorder = null;
  var recordChunks = [];
  var recordMime = '';
  var recordTimer = null;
  var recording = false;
  var MAX_RECORD_MS = 60000;

  var STATE_LABEL = {
    idle: 'READY',
    listening: 'LISTENING',
    thinking: 'THINKING',
    executing: 'EXECUTING',
    speaking: 'SPEAKING',
    error: 'ERROR',
  };

  function invoke(cmd, args) {
    if (!tauri) return Promise.reject(new Error('Tauri backend not available.'));
    return tauri.invoke(cmd, args || {});
  }

  function setState(state) {
    var s = STATE_LABEL[state] ? state : 'idle';
    pill.className = 'pill ' + s;
    statusText.textContent = STATE_LABEL[s];
    orb.className = 'orb ' + s;
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

  function stopAudio() {
    if (currentAudio) {
      try { currentAudio.pause(); } catch (e) { /* noop */ }
      currentAudio = null;
    }
  }

  function playReply(audioBase64, mimeType) {
    stopAudio();
    setState('speaking');
    subtitle.textContent = 'Speaking...';
    var audio = new Audio('data:' + (mimeType || 'audio/mpeg') + ';base64,' + audioBase64);
    currentAudio = audio;
    audio.onended = function () {
      currentAudio = null;
      setState('idle');
      greeting.textContent = 'Hey, Comrade.';
      subtitle.textContent = 'How can I help you?';
    };
    audio.onerror = function () {
      currentAudio = null;
      setState('idle');
    };
    audio.play().catch(function () { setState('idle'); });
  }

  form.addEventListener('submit', function (ev) {
    ev.preventDefault();
    var text = input.value.trim();
    if (!text) return;
    input.value = '';
    addMessage('user', text);
    resetTaskPanel(text);
    currentResponseEl = null;
    invoke('send_message', { text: text, sessionId: currentSessionId }).then(function (res) {
      if (res.session_id) { setSession(res.session_id); refreshChatList(); }
      if (res.status === 'done' || res.status === 'running') return;
      setState('idle');
      if (res.error) addMessage('comrade', 'Error: ' + res.error);
    }).catch(function (err) {
      setState('error');
      addMessage('comrade', 'Error: ' + (err && err.message ? err.message : err));
    });
  });

  cancelBtn.addEventListener('click', function () {
    stopAudio();
    stopRecording(false);
    invoke('cancel_task').catch(function () { /* noop */ });
    setState('idle');
    greeting.textContent = 'Cancelled.';
    subtitle.textContent = 'How can I help you?';
  });

  function pickMime() {
    var candidates = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4'];
    for (var i = 0; i < candidates.length; i++) {
      try {
        if (window.MediaRecorder && MediaRecorder.isTypeSupported(candidates[i])) return candidates[i];
      } catch (e) { /* noop */ }
    }
    return '';
  }

  function startRecording() {
    if (recording) return;
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      addMessage('comrade', 'Microphone is not available in this environment.');
      return;
    }
    stopAudio();
    navigator.mediaDevices.getUserMedia({ audio: true }).then(function (stream) {
      recordChunks = [];
      recordMime = pickMime();
      try {
        recorder = recordMime ? new MediaRecorder(stream, { mimeType: recordMime }) : new MediaRecorder(stream);
      } catch (e) {
        addMessage('comrade', 'Could not start recording: ' + e.message);
        stream.getTracks().forEach(function (t) { t.stop(); });
        return;
      }
      recording = true;
      setState('listening');
      subtitle.textContent = 'Listening... release to send.';
      micBtn.classList.add('live');
      recorder.ondataavailable = function (ev) {
        if (ev.data && ev.data.size > 0) recordChunks.push(ev.data);
      };
      recorder.onstop = function () {
        stream.getTracks().forEach(function (t) { t.stop(); });
      };
      recorder.start(250);
      recordTimer = setTimeout(function () { stopRecording(true); }, MAX_RECORD_MS);
    }).catch(function (err) {
      addMessage('comrade', 'Microphone blocked: ' + err.message);
    });
  }

  function stopRecording(send) {
    if (!recording || !recorder) return;
    recording = false;
    micBtn.classList.remove('live');
    if (recordTimer) { clearTimeout(recordTimer); recordTimer = null; }
    var rec = recorder;
    recorder = null;
    setState('thinking');
    subtitle.textContent = 'Understanding request...';
    rec.onstop = (function (prev) {
      return function () {
        if (prev) prev();
        if (send) sendRecording();
        else setState('idle');
      };
    })(rec.onstop);
    try { rec.stop(); } catch (e) { setState('idle'); }
  }

  function sendRecording() {
    if (recordChunks.length === 0) {
      setState('idle');
      addMessage('comrade', 'I could not hear anything. Try again.');
      return;
    }
    var type = (recordMime || 'audio/webm').split(';')[0];
    var blob = new Blob(recordChunks, { type: type });
    recordChunks = [];
    var reader = new FileReader();
    reader.onload = function () {
      var dataUrl = String(reader.result || '');
      var base64 = dataUrl.slice(dataUrl.indexOf(',') + 1);
      var format = type.indexOf('mp4') >= 0 ? 'm4a' : 'webm';
      currentResponseEl = null;
      invoke('voice_input', { audioBase64: base64, format: format, sessionId: currentSessionId }).then(function (res) {
        if (res.session_id) { setSession(res.session_id); refreshChatList(); }
        if (res.error && !res.transcript) {
          setState('error');
          addMessage('comrade', 'Error: ' + res.error);
          return;
        }
        if (res.audio_base64) playReply(res.audio_base64, res.mime_type);
        else if (res.tts_error) addMessage('comrade', '(voice reply unavailable — showing text instead)');
      }).catch(function (err) {
        setState('error');
        addMessage('comrade', 'Error: ' + (err && err.message ? err.message : err));
      });
    };
    reader.readAsDataURL(blob);
  }

  micBtn.addEventListener('mousedown', function (ev) { ev.preventDefault(); startRecording(); });
  micBtn.addEventListener('mouseup', function () { stopRecording(true); });
  micBtn.addEventListener('mouseleave', function () { if (recording) stopRecording(true); });
  micBtn.addEventListener('touchstart', function (ev) { ev.preventDefault(); startRecording(); }, { passive: false });
  micBtn.addEventListener('touchend', function (ev) { ev.preventDefault(); stopRecording(true); }, { passive: false });

  function onAgentEvent(ev) {
    if (ev.type === 'state') {
      if (ev.state === 'listening' || ev.state === 'speaking') return;
      if (currentAudio) return;
      setState(ev.state);
      if (ev.state === 'thinking') subtitle.textContent = 'Understanding request...';
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
    } else if (ev.type === 'token') {
      if (!currentResponseEl) currentResponseEl = addMessage('comrade', '');
      currentResponseEl.textContent += ev.token;
    } else if (ev.type === 'done') {
      if (!currentAudio) setState(ev.status === 'done' ? 'idle' : 'error');
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
  var memSearch = document.getElementById('mem-search');
  var memRefresh = document.getElementById('mem-refresh');
  var memCount = document.getElementById('mem-count');
  var memList = document.getElementById('mem-list');
  var searchTimer = null;

  function errMsg(err) {
    return (err && err.message ? err.message : String(err)).slice(0, 200);
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

  // boot: sidebar open, list chats, restore last session
  toggleSidebar(true);
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

