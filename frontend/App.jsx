import React, { useEffect } from "react";
import {
  PanelLeft,
  SquarePen,
  Globe,
  Settings as SettingsIcon,
  ArrowUp,
  Mic,
  AudioLines,
  Code2,
  Sparkles,
  ArrowUpRight,
  CircleHelp,
  Maximize2,
  Expand,
  MessageSquare,
  Pin,
  X,
} from "lucide-react";
import Settings from "./Settings";
import { initializeComrade } from "./app";
import logo from "./logo.png";

const prompts = [
  [
    Sparkles,
    "Make a plan",
    "Turn a big idea into clear next steps",
    "Help me turn my idea into an actionable plan. Ask me what I have in mind.",
  ],
  [
    Code2,
    "Build something",
    "A second pair of hands for your code",
    "Help me with a coding task. Ask me about the project and what I want to build.",
  ],
  [
    Globe,
    "Explore the web",
    "Find answers and connect the dots",
    "Help me research a topic on the web. Ask me what I would like to explore.",
  ],
];

export default function App() {
  useEffect(() => {
    initializeComrade();
  }, []);
  const suggest = (prompt) => {
    const input = document.getElementById("input");
    input.value = prompt;
    input.focus();
  };
  return (
    <>
      <aside id="sidebar" aria-label="Chat history">
        <div className="sidebar-brand">
          <img src={logo} alt="" />
          <span>Comrade</span>
          <span className="brand-badge">AI</span>
        </div>
        <button id="new-chat-btn" type="button" className="new-chat">
          <SquarePen size={17} />
          <span>New chat</span>
          <kbd>⌘ N</kbd>
        </button>
        <div className="sidebar-label">Your conversations</div>
        <ul id="chat-list" />
        <div className="sidebar-footer">
          <div className="workspace-avatar">C</div>
          <div>
            <strong>Personal workspace</strong>
            <span>Local-first. Yours.</span>
          </div>
          <CircleHelp size={16} aria-hidden="true" />
        </div>
      </aside>
      <div id="app">
        <header>
          <div className="head-left">
            <button
              id="sidebar-btn"
              type="button"
              className="icon-button"
              aria-label="Toggle chat sidebar"
              title="Toggle sidebar"
            >
              <PanelLeft size={19} />
            </button>
            <span className="brand">
              Comrade <span>Personal assistant</span>
            </span>
          </div>
          <div className="head-right">
            <div id="status-pill" className="pill idle" role="status">
              <span className="dot" />
              <span id="status-text">READY</span>
            </div>
            <button
              id="browser-btn"
              type="button"
              className="icon-button"
              aria-label="Toggle in-app browser"
              title="Browser"
            >
              <Globe size={19} />
            </button>
            <button
              id="settings-btn"
              type="button"
              className="icon-button"
              aria-label="Open settings"
              title="Settings"
            >
              <SettingsIcon size={19} />
            </button>
          </div>
        </header>
        <main id="workarea">
          <section id="chat-workspace" aria-label="Chat panel">
            <div className="chat-panel-bar">
              <span><MessageSquare size={16} /> Comrade</span>
              <div>
                <button id="chat-pin" type="button" aria-label="Pin chat beside browser" title="Pin chat beside browser" aria-pressed="false"><Pin size={16} /></button>
                <button id="chat-close" type="button" aria-label="Close chat panel" title="Close chat panel"><X size={16} /></button>
              </div>
            </div>
            <section id="chat-col">
              <section id="hero">
                <div className="hero-mark" id="orb">
                  <img src={logo} alt="" />
                </div>
                <span className="eyebrow">
                  A LITTLE HELP, A LOT OF POSSIBILITY
                </span>
                <h1 id="greeting">What’s on your mind?</h1>
                <p id="subtitle">
                  Think it through. Build it out. Make it happen.
                </p>
                <div id="wave" aria-hidden="true" />
                <div className="suggestions">
                  {prompts.map(([Icon, title, description, prompt]) => (
                    <button
                      key={title}
                      type="button"
                      onClick={() => suggest(prompt)}
                    >
                      <Icon size={20} />
                      <strong>
                        {title}
                        <ArrowUpRight size={15} />
                      </strong>
                      <span>{description}</span>
                    </button>
                  ))}
                </div>
              </section>
              <section id="task-panel" hidden>
                <div className="panel-head">
                  <span>Current task</span>
                  <button id="cancel-btn" type="button">
                    Stop
                  </button>
                </div>
                <div id="task-title" />
                <ul id="steps" />
              </section>
              <section id="chat" aria-label="Conversation">
                <div
                  id="messages"
                  aria-live="polite"
                  aria-relevant="additions text"
                />
              </section>
            </section>
            <footer>
              <form id="composer">
                <textarea
                  id="input"
                  rows="2"
                  placeholder="Message Comrade…"
                  aria-label="Message Comrade"
                  autoComplete="off"
                />
                <div className="composer-tools">
                  <div className="model-control">
                    <span className="model-dot" />
                    <select id="model-pick" aria-label="Model for this message">
                      <option value="nvidia/nemotron-3.5-lightning">
                        Nemotron 3.5 Lightning
                      </option>
                      <option value="deepseek-flash">DeepSeek Flash</option>
                      <option value="openai/gpt-oss-120b">GPT OSS 120B</option>
                      <option value="google/gemma-4-31b-it">Gemma 4 31B</option>
                    </select>
                  </div>
                  <div className="composer-actions">
                    <button
                      id="mic-btn"
                      type="button"
                      className="icon-button"
                      aria-label="Hold to talk"
                      title="Hold to talk, release to send"
                    >
                      <Mic size={19} />
                    </button>
                    <button
                      id="conv-btn"
                      type="button"
                      className="icon-button"
                      aria-label="Hands-free voice conversation"
                      title="Hands-free conversation"
                    >
                      <AudioLines size={19} />
                    </button>
                    <button id="send-btn" type="submit" aria-label="Send message">
                      <ArrowUp size={19} />
                    </button>
                  </div>
                </div>
              </form>
              <div id="hint">
                Your ideas, with a little backup.{" "}
                <span>Enter to send · Shift + Enter for a new line</span>
              </div>
            </footer>
          </section>
          <div
            id="browser-divider"
            hidden
            title="Drag to resize the browser pane"
          ></div>

          <aside id="browser-pane" hidden aria-label="In-app browser">
            <div className="browser-bar">
              <button id="browser-back" type="button" title="Back">
                ‹
              </button>
              <button id="browser-forward" type="button" title="Forward">
                ›
              </button>
              <button id="browser-reload" type="button" title="Reload">
                ↻
              </button>
              <form id="browser-form">
                <input
                  id="browser-url"
                  type="text"
                  placeholder="https://…"
                  autoComplete="off"
                  spellCheck="false"
                />
              </form>
              <button id="browser-shot" type="button" title="Refresh view">
                ⤾
              </button>
              <button id="browser-expand" type="button" aria-label="Make browser the main view" title="Make browser the main view" aria-pressed="false">
                <Maximize2 size={16} />
              </button>
              <button id="browser-fullscreen" type="button" aria-label="Enter fullscreen" title="Fullscreen (F11)" aria-pressed="false">
                <Expand size={16} />
              </button>
          <button id="chat-reveal" type="button" aria-label="Show chat panel" aria-controls="chat-workspace" aria-expanded="false"><MessageSquare size={18} /><span>Chat</span></button>
              <button id="browser-hide" type="button" aria-label="Hide browser" title="Hide browser">
                ✕
              </button>
            </div>
            <div id="browser-title" className="muted">
              Comrade's browser — shown only here, inside the app.
            </div>
            <div id="browser-view" className="idle">
              <img id="browser-img" alt="In-app browser view" />
              <input
                id="browser-keys"
                type="text"
                autoComplete="off"
                autoCapitalize="off"
                spellCheck="false"
                aria-label="Type into the in-app browser"
              />
            </div>
            <div id="browser-status" className="muted"></div>
          </aside>
        </main>

      </div>
      <div id="onboarding" hidden>
        <div className="onboard-card">
          <img src={logo} alt="Comrade logo" />
          <h1>Welcome to Comrade.</h1>
          <p className="muted">
            Comrade brings its own browser — it lives only inside this app
            (resizable pane, no setup). Pick the coding agents it may use.
          </p>
          <h2 className="ob-section">Coding agents</h2>
          <p className="muted">
            Comrade delegates code tasks to these. All enabled by default.
          </p>
          <div id="agent-list"></div>
          <div className="row">
            <label className="muted" htmlFor="ob-default">
              Default agent
            </label>
            <select id="ob-default"></select>
          </div>
          <button id="ob-continue" type="button">
            Start using Comrade
          </button>
          <div id="ob-status" className="muted"></div>
        </div>
      </div>

      <div
        id="perm-modal"
        hidden
        role="dialog"
        aria-modal="true"
        aria-label="Action approval"
      >
        {" "}
        <div className="perm-card">
          <h2>Comrade wants to perform:</h2>
          <pre id="perm-summary"></pre>
          <div className="perm-actions">
            <button id="perm-cancel" type="button">
              Cancel
            </button>
            <button id="perm-allow" type="button">
              Allow
            </button>
          </div>
        </div>
      </div>

      <Settings />
    </>
  );
}
