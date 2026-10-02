import React, { useEffect, useRef, useState } from "react";
import {
  Palette,
  Globe,
  AudioLines,
  Terminal,
  Brain,
  Server,
  X,
  Check,
} from "lucide-react";

const tabs = [
  [
    "appearance",
    "Appearance",
    Palette,
    "Make yourself at home",
    "A little color, a calmer workspace.",
  ],
  [
    "browser",
    "Browser",
    Globe,
    "Your built-in browser",
    "Browse together, right inside Comrade.",
  ],
  [
    "voice",
    "Voice",
    AudioLines,
    "A more natural conversation",
    "Choose how Comrade listens and speaks.",
  ],
  [
    "agents",
    "Coding agents",
    Terminal,
    "Your coding toolkit",
    "Choose the agents Comrade can delegate to.",
  ],
  [
    "memory",
    "Memory",
    Brain,
    "Context that stays with you",
    "Manage what Comrade remembers and import your chats.",
  ],
  [
    "service",
    "Connections",
    Server,
    "Connect your services",
    "Configure the backend that powers your assistant.",
  ],
];
const themes = [
  { id: "mocha", name: "Mocha", description: "Cozy & dark" },
  { id: "frappe", name: "Frappé", description: "Soft & muted" },
  { id: "latte", name: "Latte", description: "Fresh & light" },
];
export function getTheme() {
  try {
    const saved = localStorage.getItem("comrade-theme");
    return themes.some((t) => t.id === saved) ? saved : "mocha";
  } catch {
    return "mocha";
  }
}

export default function Settings() {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState("appearance");
  const [theme, setTheme] = useState(getTheme);
  const dialog = useRef(null);
  const previousFocus = useRef(null);
  useEffect(() => {
    const handle = (event) => setOpen(event.detail);
    window.addEventListener("comrade:settings", handle);
    return () => window.removeEventListener("comrade:settings", handle);
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("comrade-theme", theme);
    } catch {
      /* Storage can be unavailable. */
    }
  }, [theme]);
  useEffect(() => {
    if (!open) return;
    previousFocus.current = document.activeElement;
    const app = document.getElementById("app");
    const sidebar = document.getElementById("sidebar");
    app.inert = true;
    sidebar.inert = true;
    dialog.current.focus();
    const keyboard = (event) => {
      // A permission request takes priority over settings.
      if (!document.getElementById("perm-modal").hidden) return;
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false);
      }
      if (event.key === "Tab") {
        const controls = [
          ...dialog.current.querySelectorAll(
            'button, input, select, textarea, [tabindex="0"]',
          ),
        ].filter((el) => !el.disabled && el.getClientRects().length);
        const first = controls[0],
          last = controls.at(-1);
        if (
          event.shiftKey &&
          (document.activeElement === first ||
            document.activeElement === dialog.current)
        ) {
          event.preventDefault();
          last?.focus();
        } else if (
          !event.shiftKey &&
          (document.activeElement === last ||
            document.activeElement === dialog.current)
        ) {
          event.preventDefault();
          first?.focus();
        }
      }
    };
    document.addEventListener("keydown", keyboard);
    return () => {
      app.inert = false;
      sidebar.inert = false;
      document.removeEventListener("keydown", keyboard);
      previousFocus.current?.focus();
    };
  }, [open]);
  const selected = tabs.find((t) => t[0] === tab);
  return (
    <div
      id="settings"
      className="modal-backdrop"
      hidden={!open}
      onClick={(event) => {
        if (event.target === event.currentTarget) setOpen(false);
      }}
    >
      <div
        className="settings-dialog"
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        tabIndex={-1}
      >
        <div className="settings-head">
          <h2 id="settings-title">Settings</h2>
          <button
            id="settings-close"
            type="button"
            className="icon-button"
            aria-label="Close settings"
            onClick={() => setOpen(false)}
          >
            <X size={19} />
          </button>
        </div>
        <div className="settings-layout">
          <nav className="settings-nav" aria-label="Settings categories">
            {tabs.map(([id, label, Icon]) => (
              <button
                key={id}
                type="button"
                aria-current={tab === id ? "page" : undefined}
                className={tab === id ? "selected" : ""}
                onClick={() => setTab(id)}
              >
                <Icon size={17} />
                <span>{label}</span>
              </button>
            ))}
            <div className="settings-version">
              COMRADE<span>Your personal AI workspace</span>
            </div>
          </nav>
          <div className="settings-body">
            <div className="settings-intro">
              <span className="eyebrow">{selected[1]}</span>
              <h3>{selected[3]}</h3>
              <p className="muted">{selected[4]}</p>
            </div>
            <section hidden={tab !== "appearance"} aria-label="Appearance">
              <h4>Color theme</h4>
              <div className="theme-options">
                {themes.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    className={
                      "theme-card " + (theme === t.id ? "selected" : "")
                    }
                    aria-pressed={theme === t.id}
                    onClick={() => setTheme(t.id)}
                  >
                    <div className={"theme-preview preview-" + t.id}>
                      <div className="preview-sidebar">
                        <i />
                        <i />
                        <i />
                      </div>
                      <div className="preview-main">
                        <span />
                        <i />
                        <i />
                        <b />
                      </div>
                    </div>
                    <div className="theme-name">
                      {t.name}
                      {theme === t.id && <Check size={15} />}
                    </div>
                    <span className="muted">{t.description}</span>
                  </button>
                ))}
              </div>
              <p className="setting-note">
                Catppuccin colors. Theme changes are saved automatically on this
                device.
              </p>
              <div className="info-card">
                <Palette size={20} />
                <div>
                  <strong>A space that feels like you</strong>
                  <p>Soft pastels, thoughtful contrast, and room to focus.</p>
                </div>
              </div>
            </section>
            <section hidden={tab !== "browser"} aria-label="Browser">
              <div className="setting-group">
                <h4>In-app browser</h4>
                <p className="muted">
                  Your built-in browser shares a tab with Comrade. Open it
                  alongside your chat, then click, type, or scroll.
                </p>
                <div className="row checks">
                  <label className="check">
                    <input id="pref-autoshow" type="checkbox" /> Auto-show on
                    browser use
                  </label>
                </div>
                <div className="row checks">
                  <label className="check">
                    <input id="pref-adblock" type="checkbox" defaultChecked /> Block ads and trackers
                  </label>
                </div>
                <p className="muted">Powered by Brave’s blocking engine with EasyList and EasyPrivacy. On by default. Changes apply to new requests; reload the page to restore blocked content.</p>
                <div className="row">
                  <label className="muted" htmlFor="pref-bwidth">
                    Pane width <span id="pref-bwidth-val"></span>
                  </label>
                  <input
                    id="pref-bwidth"
                    type="range"
                    min="20"
                    max="70"
                    step="1"
                  />
                </div>
                <div className="row">
                  <button id="pane-open-btn" type="button">
                    Open pane
                  </button>
                  <button id="pane-shot-btn" type="button">
                    Refresh view
                  </button>
                  <button id="pane-close-btn" type="button">
                    Stop browser
                  </button>
                </div>
              </div>
            </section>
            <section hidden={tab !== "voice"} aria-label="Voice">
              <div className="setting-group">
                <h4>Voice</h4>
                <label className="check setting-toggle">
                  <input id="pref-autoplay" type="checkbox" /> Play voice
                  replies automatically
                </label>
                <label className="muted" htmlFor="voice-backend">
                  Voice backend
                </label>
                <select id="voice-backend">
                  <option value="server">
                    Server — use your configured connection
                  </option>
                  <option value="local">
                    Local — offline after model download
                  </option>
                </select>
                <label className="muted" htmlFor="voice-mic">
                  Microphone
                </label>
                <select id="voice-mic"></select>
                <div className="row">
                  <button id="voice-models-btn" type="button">
                    Check models
                  </button>
                  <button id="voice-dl-btn" type="button">
                    Download
                  </button>
                </div>
                <div id="model-status" className="muted"></div>
                <div className="progress">
                  <div id="model-progress"></div>
                </div>
                <div className="row vgrid">
                  <label className="muted">
                    VAD threshold{" "}
                    <input
                      id="voice-vad-t"
                      type="number"
                      min="0.1"
                      max="0.9"
                      step="0.05"
                    />
                  </label>
                  <label className="muted">
                    Silence ms{" "}
                    <input
                      id="voice-silence"
                      type="number"
                      min="200"
                      max="3000"
                      step="50"
                    />
                  </label>
                  <label className="muted">
                    TTS voice <input id="voice-tts-voice" type="text" />
                  </label>
                  <label className="muted">
                    TTS speed{" "}
                    <input
                      id="voice-tts-speed"
                      type="number"
                      min="0.25"
                      max="4"
                      step="0.05"
                    />
                  </label>
                </div>
              </div>
            </section>
            <section hidden={tab !== "agents"} aria-label="Coding agents">
              <div className="setting-group">
                <h4>Coding agents</h4>
                <div id="code-agent-list"></div>
                <div className="row">
                  <label className="muted" htmlFor="code-default">
                    Default
                  </label>
                  <select id="code-default"></select>
                </div>
              </div>
            </section>
            <section hidden={tab !== "memory"} aria-label="Memory">
              <div className="setting-group">
                <h4>Add memory</h4>
                <textarea
                  id="mem-text"
                  rows="4"
                  placeholder="e.g. I prefer dark mode. My Co-Founder repo is at /home/..."
                ></textarea>
                <div className="row">
                  <select id="mem-kind">
                    <option value="fact">fact</option>
                    <option value="preference">preference</option>
                    <option value="note">note</option>
                    <option value="project">project (Name -&gt; /path)</option>
                  </select>
                  <button id="mem-save" type="button">
                    Save
                  </button>
                </div>
                <div id="mem-status" className="muted"></div>
              </div>
              <div className="setting-group">
                <h4>Import from ChatGPT</h4>
                <p className="muted">
                  Export from ChatGPT Settings → Data controls → Export, then
                  pick the <code>conversations.json</code> file.
                </p>
                <div className="row">
                  <input
                    id="import-file"
                    type="file"
                    accept=".json,application/json"
                  />
                  <button id="import-btn" type="button">
                    Import
                  </button>
                </div>
                <div id="import-status" className="muted"></div>
              </div>
              <div className="setting-group">
                <h4>Memories</h4>
                <div className="row">
                  <input
                    id="mem-search"
                    type="text"
                    placeholder="Search memories..."
                  />
                  <button id="mem-refresh" type="button" title="Refresh list">
                    ↻
                  </button>
                </div>
                <div id="mem-count" className="muted"></div>
                <ul id="mem-list"></ul>
              </div>
            </section>
            <section hidden={tab !== "service"} aria-label="Connections">
              <div className="setting-group">
                <h4>Service backend (optional)</h4>
                <p className="muted">
                  Connect an OpenAI-compatible service for chat, embeddings, and
                  voice. Changes apply when you save; changing embedding
                  dimensions requires a restart.
                </p>
                <div className="row checks">
                  <label className="check">
                    <input id="srv-enabled" type="checkbox" /> Use service
                    server
                  </label>
                  <button id="srv-test" type="button">
                    Test
                  </button>
                </div>
                <div id="srv-status" className="muted"></div>
                <label className="muted" htmlFor="srv-url">
                  Server URL
                </label>
                <input
                  id="srv-url"
                  type="text"
                  placeholder="http://localhost:8000"
                  autoComplete="off"
                  spellCheck="false"
                />
                <label className="muted" htmlFor="srv-key">
                  API key (optional)
                </label>
                <input
                  id="srv-key"
                  type="password"
                  placeholder="bearer key, if your server needs one"
                  autoComplete="off"
                />
                <div className="row vgrid">
                  <label className="muted">
                    LLM model
                    <select id="srv-llm">
                      <option value="nvidia/nemotron-3.5-lightning">
                        nvidia/nemotron-3.5-lightning
                      </option>
                      <option value="deepseek-flash">deepseek-flash</option>
                      <option value="openai/gpt-oss-120b">
                        openai/gpt-oss-120b
                      </option>
                      <option value="google/gemma-4-31b-it">
                        google/gemma-4-31b-it
                      </option>
                    </select>
                  </label>
                  <label className="muted">
                    Embedding model <input id="srv-emb" type="text" />
                  </label>
                  <label className="muted">
                    Embedding dim{" "}
                    <input id="srv-dim" type="number" min="1" step="1" />
                  </label>
                  <label className="muted">
                    STT model <input id="srv-stt" type="text" />
                  </label>
                  <label className="muted">
                    TTS voice <input id="srv-tts" type="text" />
                  </label>
                </div>
              </div>
              <div className="setting-group">
                <h4>Brain</h4>
                <div id="model-info" className="muted">
                  loading...
                </div>
              </div>
            </section>
          </div>
        </div>
        <div className="settings-footer">
          <div>
            <div id="pref-status" role="status" className="muted" />
            <div
              id="pref-file"
              className="muted"
              hidden={tab === "appearance"}
            />
            {tab === "appearance" && (
              <span className="muted">Your theme is saved automatically.</span>
            )}
          </div>
          <button
            id="pref-save"
            hidden={tab === "appearance"}
            className="primary-button"
            type="button"
          >
            Save preferences
          </button>
          {tab === "appearance" && (
            <button
              className="primary-button"
              type="button"
              onClick={() => setOpen(false)}
            >
              Done
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
