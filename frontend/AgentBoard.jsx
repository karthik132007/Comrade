import React, { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Code2, Plus, Activity, Square, RefreshCw, Folder, FolderOpen, ArrowLeft, ChevronRight } from "lucide-react";

const invoke = (command, args = {}) => {
  const bridge = window.__TAURI__?.core?.invoke;
  return bridge ? bridge(command, args) : Promise.reject(new Error("Open the desktop app to select a project and run coding agents."));
};
const active = (status) => status === "queued" || status === "running";
const readable = (err) => typeof err === "string" ? err : err?.message || "The action failed.";
const newTask = (id) => ({ id, task: "", agent: "", model: "", depends_on: [] });
const projectName = (path) => path.split(/[\\/]/).filter(Boolean).at(-1) || path;
const PROJECTS_KEY = "comrade-orch-projects";
function savedProjects() {
  try {
    const saved = JSON.parse(window.localStorage.getItem(PROJECTS_KEY) || "[]");
    return Array.isArray(saved) ? [...new Set(saved.filter(p => typeof p === "string" && p.length))] : [];
  } catch { return []; }
}

export default function AgentBoard({ open, setOpen }) {
  const [projects, setProjects] = useState(savedProjects);
  const [directory, setDirectory] = useState(() => savedProjects()[0] || "");
  const [runtime, setRuntime] = useState(null);
  const [runs, setRuns] = useState([]);
  const [selected, setSelected] = useState(null);
  const [title, setTitle] = useState("");
  const [tasks, setTasks] = useState([newTask("task-1")]);
  const [parallel, setParallel] = useState(2);
  const [timeout, setTimeout] = useState(600);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const counter = useRef(1);
  const refreshSeq = useRef(0);
  const workspaceRef = useRef(null);
  const launchRef = useRef(null);
  const mounted = useRef(false);
  const run = runs.find((r) => r.id === selected);
  const projectRuns = runs.filter(r => !directory || r.plan.working_dir === directory);

  const refresh = async () => {
    const seq = ++refreshSeq.current;
    const list = await invoke("coding_runs");
    let next = Array.isArray(list) ? list : [];
    if (selected && next.some(r => r.id === selected)) {
      const detail = await invoke("coding_run", { runId: selected });
      next = next.map(r => r.id === selected ? detail : r);
    }
    if (mounted.current && seq === refreshSeq.current) setRuns(next);
  };
  const inspect = async () => {
    try { setRuntime(await invoke("coding_runtime")); } catch (e) { setRuntime({ ready: false, message: readable(e), agents: [] }); }
  };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    try { window.localStorage.setItem(PROJECTS_KEY, JSON.stringify(projects)); } catch { /* Projects remain usable when storage is unavailable. */ }
  }, [projects]);
  useEffect(() => {
    if (!open) return;
    let alive = true;
    let timer;
    const poll = async () => {
      try { await refresh(); } catch (e) { if (alive) setError(readable(e)); }
      if (alive) timer = window.setTimeout(poll, 1200);
    };
    poll(); inspect();
    return () => { alive = false; window.clearTimeout(timer); ++refreshSeq.current; };
  }, [open, selected]);
  useEffect(() => {
    if (!open) return;
    workspaceRef.current?.focus();
    const onKey = (event) => {
      if (document.getElementById("perm-modal")?.hidden === false || document.getElementById("settings")?.hidden === false || picking) return;
      if (event.key === "Escape") {
        event.preventDefault(); event.stopImmediatePropagation(); setOpen(false);
        window.requestAnimationFrame(() => launchRef.current?.focus());
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [open, picking, setOpen]);
  const resetPlan = () => {
    setSelected(null); setTitle(""); setTasks([newTask("task-1")]); counter.current = 1; setError("");
  };
  const selectProject = (path) => {
    if (path !== directory) resetPlan();
    else { setSelected(null); setError(""); }
    setDirectory(path);
    setProjects(rows => [path, ...rows.filter(p => p !== path)]);
    setOpen(true);
    if (window.matchMedia("(max-width: 900px)").matches) {
      window.requestAnimationFrame(() => {
        document.getElementById("sidebar").hidden = true;
        document.body.classList.remove("sidebar-open");
      });
    }
  };
  const pickProject = async () => {
    setOpen(true); setPicking(true); setError("");
    try {
      const project = await invoke("coding_pick_project", { initialDir: directory || null });
      if (project?.path) selectProject(project.path);
    } catch (e) { setError(readable(e)); }
    finally { setPicking(false); }
  };
  const action = async (fn) => {
    setBusy(true); setError("");
    try { await fn(); await refresh(); } catch (e) { setError(readable(e)); }
    finally { setBusy(false); }
  };
  const updateTask = (id, update) => setTasks(rows => rows.map(t => t.id === id ? { ...t, ...update } : t));
  const submit = (event) => {
    event.preventDefault();
    if (!directory) return;
    action(async () => {
      const result = await invoke("coding_start", { plan: { title: title.trim(), working_dir: directory, tasks, max_parallel: Number(parallel), timeout_secs: Number(timeout) } });
      setSelected(result.id);
    });
  };
  const askComrade = () => {
    const goal = title.trim() || tasks.map(t => t.task.trim()).filter(Boolean).join("; ");
    if (!goal || !directory) return;
    window.dispatchEvent(new CustomEvent("comrade:orchestration-request", { detail: { text: goal, projectDir: directory } }));
    setOpen(false);
  };
  const viewRun = (r) => { selectProject(r.plan.working_dir); setSelected(r.id); };
  return <>
    <section className="orch-sidebar" aria-label="Comrade Orch projects">
      <button ref={launchRef} type="button" className={`orch-nav ${open ? "selected" : ""}`} aria-current={open ? "page" : undefined} aria-controls="orch-workspace" onClick={() => { setOpen(true); setError(""); }}><Code2 size={17} /><span>Comrade Orch</span><ChevronRight size={15} /></button>
      <button type="button" className="orch-add-project" disabled={picking || busy} onClick={pickProject}><Plus size={15} /><span>{picking ? "Selecting…" : "Add project"}</span></button>
      {!!projects.length && <nav className="orch-projects" aria-label="Orchestration projects">{projects.map(path => <button key={path} type="button" disabled={busy || picking} title={path} aria-label={`Open project ${projectName(path)}`} className={open && directory === path ? "selected" : ""} aria-current={open && directory === path ? "page" : undefined} onClick={() => selectProject(path)}><Folder size={15} /><span>{projectName(path)}</span></button>)}</nav>}
    </section>
    {open && createPortal(<section ref={workspaceRef} tabIndex={-1} className="agent-board orch-page" aria-label="Comrade Orch">
      <div className="agent-board-heading"><div><span className="eyebrow">YOUR CODING WORKSPACE</span><h2><Code2 size={21} /> Comrade Orch</h2></div><button type="button" className="icon-button" aria-label="Back to chat" title="Back to chat" onClick={() => window.dispatchEvent(new Event("comrade:show-chat"))}><ArrowLeft size={20} /></button></div>
      <div className="agent-board-body">
        <nav className="agent-runs" aria-label="Coding runs"><button type="button" className={selected ? "" : "selected"} onClick={resetPlan}><Plus size={16} /> New task plan</button>
          <p className="orch-runs-label">{directory ? `${projectName(directory)} runs` : "Recent runs"}</p>
          {!projectRuns.length && <p>Your delegated tasks will appear here.</p>}
          {projectRuns.map(r => <button key={r.id} type="button" className={selected === r.id ? "selected" : ""} onClick={() => viewRun(r)}><strong>{r.plan.title}</strong><span className={`agent-state ${r.status}`}>{r.status.replaceAll("_", " ")} · {r.jobs.filter(j => j.status === "succeeded").length}/{r.jobs.length}</span></button>)}
        </nav>
        <div className="agent-content">
          {error && <p className="agent-error" role="alert">{error}</p>}
          {run ? <>
            <div className="agent-run-title"><div><h3>{run.plan.title}</h3><p><FolderOpen size={14} /> {run.plan.working_dir}</p></div>{active(run.status) && <button type="button" disabled={busy} onClick={() => action(() => invoke("coding_cancel", { runId: run.id, jobId: null }))}><Square size={14} /> Stop run</button>}</div>
            <div className="agent-policy"><Activity size={18} /><span>Local agents · {run.plan.max_parallel} parallel · {run.plan.timeout_secs}s per job</span></div>
            {run.jobs.map(j => <article key={j.spec.id} className="agent-job">
              <div className="agent-job-heading"><strong>{j.spec.id} <span>{j.spec.agent}{j.spec.model ? ` · ${j.spec.model}` : ""}</span></strong><span className={`agent-state ${j.status}`}>{j.status}</span></div>
              <p>{j.spec.task}</p>
              {!!j.spec.depends_on.length && <small>After {j.spec.depends_on.join(", ")}</small>}
              {j.error && <p className="agent-error">{j.error}</p>}
              <details><summary>Agent output{!j.log && " · waiting for output"}</summary><pre tabIndex="0">{j.log || "No output yet."}</pre></details>
              <div className="agent-job-actions">
                {active(j.status) && <button type="button" disabled={busy} onClick={() => action(() => invoke("coding_cancel", { runId: run.id, jobId: j.spec.id }))}><Square size={14} /> Stop job</button>}
                {j.status === "succeeded" && <small>Agent finished. See output for checks and results.</small>}
              </div>
            </article>)}
          </> : !directory ? <div className="orch-empty">
            <div className="orch-empty-icon"><FolderOpen size={30} /></div><h3>Start with your project</h3><p>Choose the folder you want to work in. Comrade will coordinate your coding agents here.</p>
            <button type="button" className="agent-primary" disabled={picking || busy} onClick={pickProject}><Plus size={16} /> {picking ? "Selecting…" : "Choose project directory"}</button><small>Your projects stay in the sidebar for next time.</small>
          </div> : <>
            <h3>What are we building?</h3><p>Ask Comrade to break down a goal, or assign the tasks yourself. Agents work directly in your project using their existing logins and permissions.</p>
            <div className={`agent-runtime ${runtime?.ready ? "ready" : ""}`} role="status"><Activity size={19} /><span>{runtime?.message || "Checking installed coding agents…"}</span><button type="button" className="icon-button" aria-label="Recheck coding runtime" onClick={inspect}><RefreshCw size={16} /></button></div>
            <form className="agent-plan-form" onSubmit={submit}>
              <div className="orch-directory"><label htmlFor="orch-project-directory">Project directory<input id="orch-project-directory" readOnly value={directory} /></label><button type="button" disabled={picking || busy} onClick={pickProject}><FolderOpen size={16} /> Change directory</button></div>
              <label>Goal<input required value={title} onChange={e => setTitle(e.target.value)} maxLength={200} placeholder="Build a settings page and test it" /></label>
              <details className="agent-run-options"><summary>Run options</summary><div className="agent-plan-options"><label>Parallel jobs<select value={parallel} onChange={e => setParallel(e.target.value)}>{[1,2,3].map(n => <option key={n}>{n}</option>)}</select></label><label>Job deadline (seconds)<input type="number" min="30" max="1800" value={timeout} onChange={e => setTimeout(e.target.value)} required /></label></div></details>
              {tasks.map((t, index) => <fieldset className="agent-task-editor" key={t.id}><legend>{t.id}</legend>
                <div className="agent-task-options"><label>Agent<select aria-label="Agent" value={t.agent} onChange={e => updateTask(t.id, {agent: e.target.value})}><option value="">Comrade default</option>{(runtime?.agents || []).map(a => <option key={a.id} value={a.id} disabled={!a.available || !a.enabled}>{a.name}{!a.available ? " · unavailable" : !a.enabled ? " · disabled" : ""}</option>)}</select></label><label>Model (optional)<input value={t.model} onChange={e => updateTask(t.id, {model: e.target.value})} placeholder="Agent default" maxLength={160} /></label></div>
                <label>Task<textarea required rows={3} value={t.task} onChange={e => updateTask(t.id, {task: e.target.value})} maxLength={16000} placeholder="Describe the change and how to verify it" /></label>
                {index > 0 && <div className="agent-dependencies"><span>Depends on</span>{tasks.slice(0,index).map(dep => <label key={dep.id}><input type="checkbox" checked={t.depends_on.includes(dep.id)} onChange={e => updateTask(t.id, {depends_on:e.target.checked ? [...t.depends_on,dep.id] : t.depends_on.filter(id => id !== dep.id)})} />{dep.id}</label>)}</div>}
                {tasks.length > 1 && <button type="button" onClick={() => setTasks(rows => rows.filter(row => row.id !== t.id).map(row => ({...row, depends_on:row.depends_on.filter(id => id !== t.id)})))}>Remove task</button>}
              </fieldset>)}
              <div className="agent-plan-actions"><button type="button" disabled={tasks.length >= 16} onClick={() => setTasks(rows => [...rows,newTask(`task-${++counter.current}`)])}><Plus size={15} /> Add task</button><button type="button" disabled={busy || picking || (!title.trim() && !tasks.some(t => t.task.trim()))} onClick={askComrade}>Let Comrade plan</button><button className="agent-primary" type="submit" disabled={busy || picking || !runtime?.ready}>{busy ? "Starting…" : "Start run"}</button></div>
            </form>
          </>}
        </div>
      </div>
    </section>, document.getElementById("orch-workspace"))}
  </>;
}
