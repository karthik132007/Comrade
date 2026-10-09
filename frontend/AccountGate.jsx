import React, { useEffect, useRef, useState } from "react";
import { ArrowUpRight, LoaderCircle, ShieldCheck, LogIn } from "lucide-react";
import App from "./App";
import logo from "./logo.png";

const accountPage = "https://www.roviumlabs.me/products/comrade";
const invoke = (command, args = {}) => window.__TAURI__?.core?.invoke(command, args);
const readable = (error) => typeof error === "string" ? error : error?.message || "Please try again.";

export default function AccountGate() {
  const desktop = !!window.__TAURI__?.core;
  const [account, setAccount] = useState(null);
  const [loading, setLoading] = useState(desktop);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [login, setLogin] = useState(null);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [passwordForm, setPasswordForm] = useState(true);
  const sequence = useRef(0);
  const mounted = useRef(true);
  const usedApp = useRef(false);

  const accept = (next) => {
    if (!mounted.current) return;
    setAccount(next);
    if (next?.authenticated) {
      // The imperative desktop controller attaches document listeners once.
      // A fresh document clears them when signing back in after a lock.
      if (usedApp.current) window.location.reload();
      usedApp.current = true;
      setLogin(null);
      setPassword("");
    }
  };

  useEffect(() => {
    mounted.current = true;
    if (desktop) invoke("account_status").then(accept).catch(e => setError(readable(e))).finally(() => setLoading(false));
    let unlisten;
    let cancelled = false;
    const lock = () => {
      ++sequence.current;
      setAccount(null); setLogin(null); setBusy(false);
      setError("Your session ended. Sign in again to continue.");
    };
    if (window.__TAURI__?.event) {
      window.__TAURI__.event.listen("account-event", lock).then(stop => {
        if (cancelled) stop(); else unlisten = stop;
      });
    }
    return () => { mounted.current = false; cancelled = true; unlisten?.(); ++sequence.current; };
  }, []);

  useEffect(() => {
    if (!login) return;
    const version = sequence.current;
    const deadline = Date.now() + login.expires_in * 1000;
    let timer;
    let stopped = false;
    const poll = async () => {
      if (stopped || version !== sequence.current) return;
      if (Date.now() >= deadline) {
        setLogin(null); setError("This sign-in request expired. Start a new one.");
        invoke("account_cancel_login").catch(() => {}); return;
      }
      try {
        const status = await invoke("account_poll_login");
        if (stopped || version !== sequence.current) return;
        if (status.authenticated) { accept(status); return; }
        if (!status.pending) { setLogin(null); setError("This request ended. Please start again."); return; }
        setError("");
      } catch (e) {
        if (stopped || version !== sequence.current) return;
        setError(readable(e));
      }
      timer = window.setTimeout(poll, Math.max(3, login.interval || 3) * 1000);
    };
    timer = window.setTimeout(poll, Math.max(3, login.interval || 3) * 1000);
    return () => { stopped = true; window.clearTimeout(timer); };
  }, [login]);

  const start = async () => {
    setBusy(true); setError("");
    const version = ++sequence.current;
    try {
      const request = await invoke("account_start_login");
      if (version !== sequence.current || !mounted.current) return;
      setLogin(request);
      await invoke("account_open_login");
    } catch (e) { if (version === sequence.current) setError(readable(e)); }
    finally { if (version === sequence.current) setBusy(false); }
  };
  const signIn = async (event) => {
    event.preventDefault();
    setBusy(true); setError(""); ++sequence.current; setLogin(null);
    try {
      await invoke("account_cancel_login");
      const next = await invoke("account_sign_in", { email: email.trim(), password });
      accept(next);
    } catch (e) { setError(readable(e)); }
    finally { setPassword(""); setBusy(false); }
  };
  const signOut = async () => {
    setBusy(true); setError("");
    try { await invoke("account_logout"); window.location.reload(); }
    catch (e) { setError(readable(e)); setBusy(false); }
  };

  if (account?.authenticated) return <App account={account.user} onSignOut={signOut} signingOut={busy} accountError={error} />;
  return <main className="account-gate" aria-label="Comrade account">
    <section className="account-card">
      <img className="account-logo" src={logo} alt="Comrade" />
      <span className="eyebrow">YOUR COMRADE, READY WHEN YOU ARE</span>
      <h1>{loading ? "Opening your workspace…" : "Sign in to Comrade"}</h1>
      <p>One Rovium Labs account for your desktop agent. Create an account or sign in to get started.</p>
      <div className="account-testing"><ShieldCheck size={17} /> Unlimited access during internal testing</div>
      {loading ? <LoaderCircle className="account-spinner" aria-label="Checking account" /> : <>
        {desktop ? <>
          {!passwordForm && <button className="account-secondary" type="button" onClick={() => setPasswordForm(true)} disabled={busy}>Sign in here with email</button>}
          {passwordForm && <form className="account-form" onSubmit={signIn}>
            <label>Email<input type="email" autoComplete="username" required maxLength={254} value={email} onChange={e => setEmail(e.target.value)} disabled={busy} /></label>
            <label>Password<input type="password" autoComplete="current-password" required value={password} onChange={e => setPassword(e.target.value)} disabled={busy} /></label>
            <button className="account-primary" type="submit" disabled={busy}>{busy ? "Signing in…" : "Sign in"}</button>
          </form>}
          {account?.browser_login_available && <button className="account-secondary" type="button" disabled={busy} onClick={start}>
            <LogIn size={18} /> {busy ? "Please wait…" : login ? "Start a new browser sign-in" : "Continue in browser"}<ArrowUpRight size={18} />
          </button>}
          {login && <div className="account-pairing" role="status">
            <span>Confirm this code on the Rovium Labs page</span><strong>{login.user_code}</strong>
            <p>Waiting for you to connect this desktop. Keep this window open.</p>
            <button type="button" onClick={() => invoke("account_open_login").catch(e => setError(readable(e)))}>Reopen sign-in page</button>
            <button type="button" onClick={async () => { ++sequence.current; setLogin(null); await invoke("account_cancel_login").catch(e => setError(readable(e))); }}>Cancel</button>
          </div>}
          <button className="account-secondary" type="button" onClick={() => invoke("account_open_page").catch(e => setError(readable(e)))}>Create account or reset password <ArrowUpRight size={15} /></button>
        </> : <>
          <a className="account-primary" href={accountPage} target="_blank" rel="noreferrer">Create account or sign in <ArrowUpRight size={18} /></a>
          <p className="account-preview">Browser preview. Open the Comrade desktop app to use chat, voice, browser tools, and coding agents.</p>
        </>}
      </>}
      {error && <p className="account-error" role="alert">{error}</p>}
      <p className="account-note">Conversation history and project files are stored on this device. Hosted AI requests go through the Comrade service.</p>
    </section>
  </main>;
}
