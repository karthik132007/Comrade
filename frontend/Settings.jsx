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
import mikuBackdrop from "./miku.jpg";

export const MIKU_BG = mikuBackdrop;

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
  { id: "tokyo-night", name: "Tokyo Night", description: "Neon & dark" },
  {
    id: "high-contrast",
    name: "High Contrast",
    description: "Bold & clear",
  },
  { id: "github", name: "GitHub", description: "Clean & light" },
  { id: "miku", name: "Miku", description: "Teal & electric" },
];
export function getTheme() {
  try {
    const saved = localStorage.getItem("comrade-theme");
    if (saved === "manga" || saved === "anime") return "miku";
    return themes.some((t) => t.id === saved) ? saved : "mocha";
  } catch {
    return "mocha";
  }
}

const BG_IMAGE_KEY = "comrade-bg-image";
const BG_OPACITY_KEY = "comrade-bg-opacity";
const BG_DEFAULT_OPACITY = 0.35;

export function getBackgroundImage() {
  try {
    const saved = localStorage.getItem(BG_IMAGE_KEY);
    return saved || null;
  } catch {
    return null;
  }
}

export function getBackgroundOpacity() {
  try {
    const saved = parseFloat(localStorage.getItem(BG_OPACITY_KEY));
    if (Number.isFinite(saved)) return Math.min(1, Math.max(0, saved));
  } catch {
    /* Storage can be unavailable. */
  }
  return BG_DEFAULT_OPACITY;
}

export function applyBackground(image, opacity) {
  try {
    const root = document.documentElement;
    const value =
      typeof opacity === "number" && Number.isFinite(opacity)
        ? Math.min(1, Math.max(0, opacity))
        : BG_DEFAULT_OPACITY;
    if (image) {
      const safe = String(image).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
      root.style.setProperty("--app-bg-image", `url("${safe}")`);
      root.style.setProperty("--app-bg-opacity", String(value));
      if (document.body) document.body.classList.add("has-custom-bg");
    } else {
      root.style.removeProperty("--app-bg-image");
      root.style.removeProperty("--app-bg-opacity");
      if (document.body) document.body.classList.remove("has-custom-bg");
    }
  } catch {
    /* Background is decorative; never break startup. */
  }
}

function readFileAsDataUrl(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("decode failed"));
    img.src = src;
  });
}

async function processImageFile(file) {
  const raw = await readFileAsDataUrl(file);
  if (typeof raw !== "string") throw new Error("unreadable");
  if (raw.length < 2_000_000) return raw;
  const img = await loadImage(raw);
  const width = img.naturalWidth || img.width;
  const height = img.naturalHeight || img.height;
  const max = 1920;
  const scale = Math.min(1, max / Math.max(width, height));
  if (scale >= 1) return raw;
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(width * scale);
  canvas.height = Math.round(height * scale);
  const ctx = canvas.getContext("2d");
  ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
  return canvas.toDataURL("image/jpeg", 0.82);
}

export default function Settings() {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState("appearance");
  const [theme, setTheme] = useState(getTheme);
  const [bgImage, setBgImage] = useState(getBackgroundImage);
  const [bgOpacity, setBgOpacity] = useState(getBackgroundOpacity);
  const [bgUrlDraft, setBgUrlDraft] = useState(() => {
    const current = getBackgroundImage();
    return current && !current.startsWith("data:") ? current : "";
  });
  const [bgError, setBgError] = useState("");
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
    applyBackground(bgImage || (theme === "miku" ? MIKU_BG : null), bgOpacity);
    try {
      if (bgImage) localStorage.setItem(BG_IMAGE_KEY, bgImage);
      else localStorage.removeItem(BG_IMAGE_KEY);
      localStorage.setItem(BG_OPACITY_KEY, String(bgOpacity));
    } catch {
      setBgError("Background applied, but it could not be saved.");
    }
  }, [bgImage, bgOpacity, theme]);
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
  const handleBgFile = async (event) => {
    const file = event.target.files && event.target.files[0];
    event.target.value = "";
    if (!file) return;
    if (!file.type.startsWith("image/")) {
      setBgError("Please choose an image file.");
      return;
    }
    if (file.size > 12_000_000) {
      setBgError("That image is too large (max ~12MB).");
      return;
    }
    try {
      setBgError("");
      setBgImage(await processImageFile(file));
    } catch {
      setBgError("Could not read that image.");
    }
  };
  const handleBgUrlApply = () => {
    const value = bgUrlDraft.trim();
    if (!value) return;
    if (!/^https?:\/\/.+/i.test(value) && !value.startsWith("data:image/")) {
      setBgError("Use an http(s) image URL.");
      return;
    }
    setBgError("");
    setBgImage(value);
  };
  const clearBgImage = () => {
    setBgImage(null);
    setBgUrlDraft("");
    setBgError("");
  };
  const effectiveBg = bgImage || (theme === "miku" ? MIKU_BG : null);
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
              <div className="setting-group bg-group">
                <h4>Background image</h4>
                <p className="muted">
                  Add a personal backdrop behind the workspace. Opacity
                  controls how visible it is.
                </p>
                <div className="bg-preview" aria-live="polite">
                  {effectiveBg ? (
                    <img src={effectiveBg} alt="Background preview" />
                  ) : (
                    <span>No background — theme color only.</span>
                  )}
                </div>
                <div className="row">
                  <label className="muted" htmlFor="bg-file">
                    Image file
                  </label>
                  <input
                    id="bg-file"
                    type="file"
                    accept="image/*"
                    onChange={handleBgFile}
                  />
                  {bgImage && (
                    <button type="button" onClick={clearBgImage}>
                      Remove
                    </button>
                  )}
                </div>
                <div className="bg-url-row">
                  <input
                    id="bg-url"
                    type="url"
                    placeholder="Or paste an image URL, https://…"
                    value={bgUrlDraft}
                    onChange={(event) => setBgUrlDraft(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        handleBgUrlApply();
                      }
                    }}
                    autoComplete="off"
                    spellCheck="false"
                  />
                  <button type="button" onClick={handleBgUrlApply}>
                    Apply
                  </button>
                </div>
                <div className="row">
                  <label className="muted" htmlFor="bg-opacity">
                    Opacity{" "}
                    <span id="bg-opacity-val">
                      {Math.round(bgOpacity * 100)}%
                    </span>
                  </label>
                  <input
                    id="bg-opacity"
                    type="range"
                    min="0"
                    max="100"
                    step="1"
                    value={Math.round(bgOpacity * 100)}
                    onChange={(event) =>
                      setBgOpacity(Number(event.target.value) / 100)
                    }
                    disabled={!effectiveBg}
                  />
                </div>
                {theme === "miku" && !bgImage && (
                  <p className="muted">
                    Miku brings her own backdrop — upload your own above to
                    replace it.
                  </p>
                )}
                {bgError && (
                  <div className="muted" role="status">
                    {bgError}
                  </div>
                )}
              </div>
              <p className="setting-note">
                Theme changes are saved automatically on this device.
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
                <p className="muted">uBlock Origin Lite is bundled and runs in Complete mode by default. Reload the page after changing protection to apply its scriptlets and restore blocked content.</p>
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
                <h4>Comrade service</h4>
                <p className="muted">
                  Your signed-in account connects chat, memory, and server voice.
                  Changing the server requires signing in again. Changing
                  embedding dimensions requires a restart.
                </p>
                <div className="row checks">
                  <label className="check">
                    <input id="srv-enabled" type="checkbox" defaultChecked disabled /> Account service enabled
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
                <input
                  id="srv-key"
                  type="hidden"
                  value=""
                  readOnly
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
