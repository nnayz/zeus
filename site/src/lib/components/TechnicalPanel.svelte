<script lang="ts">
  type Concept = 'sessions' | 'worktrees' | 'remote' | 'engine' | 'orchestration';
  type Status = 'working' | 'needs' | 'done';

  type Session = {
    id: string;
    title: string;
    kind: string;
    status: Status;
    child?: boolean;
    host: string;
    cwd: string;
    body: string[];
    prompt: string;
  };

  type CodeLine = { text: string; comment?: boolean };

  const concepts: { id: Concept; label: string }[] = [
    { id: 'sessions', label: 'Sessions' },
    { id: 'worktrees', label: 'Worktrees' },
    { id: 'remote', label: 'Remote' },
    { id: 'engine', label: 'Engine' },
    { id: 'orchestration', label: 'Orchestration' }
  ];

  const sessions: Session[] = [
    {
      id: 'signed-dmg',
      title: 'Prepare the signed DMG',
      kind: 'claude',
      status: 'working',
      host: 'This Mac',
      cwd: '~/Desktop/zeus/zeus',
      body: [
        'Claude Code  ·  local PTY',
        '',
        'Reading PACKAGING.md and the notarization checklist.',
        'Next: Developer ID is still required before the updater can run.'
      ],
      prompt: ''
    },
    {
      id: 'sidebar',
      title: 'Polish the left sidebar',
      kind: 'codex',
      status: 'needs',
      host: 'This Mac',
      cwd: '.zeus/worktrees/sidebar-craft',
      body: [
        'OpenAI Codex (v0.148.0)',
        'model: gpt-5.6-codex   xhigh',
        'directory: .zeus/worktrees/sidebar-craft',
        '',
        'Tightened session rows to 28px and hid actions until hover.',
        'cargo test -p zeus-app -- sidebar   ok',
        '',
        'Needs you: allow cargo fmt on the working tree?'
      ],
      prompt: 'Ask Codex to do anything'
    },
    {
      id: 'switcher',
      title: 'Fix project switching',
      kind: 'codex',
      status: 'working',
      child: true,
      host: 'This Mac',
      cwd: '.zeus/worktrees/sidebar-craft',
      body: [
        'spawn_agent  kind=codex  name=project-switch',
        'parent: polish-sidebar',
        '',
        'Inspecting workspace restore on startup.',
        'Status: working'
      ],
      prompt: ''
    },
    {
      id: 'rails',
      title: 'Check the rails',
      kind: 'shell',
      status: 'done',
      child: true,
      host: 'This Mac',
      cwd: '.zeus/worktrees/sidebar-craft',
      body: [
        '$ cargo test -p zeus-app -- sidebar',
        'running 14 tests',
        'test sidebar::view ... ok',
        '',
        'exited 0'
      ],
      prompt: ''
    },
    {
      id: 'dev-server',
      title: 'Dev server · localhost',
      kind: 'shell',
      status: 'working',
      host: 'This Mac',
      cwd: '~/Desktop/zeus/site',
      body: [
        '$ bun run dev',
        'VITE  ready',
        'Local: http://localhost:5173/'
      ],
      prompt: ''
    }
  ];

  const files: Record<Concept, { id: string; label: string }[]> = {
    sessions: [
      { id: 'sidebar', label: 'sidebar-craft' },
      { id: 'signed-dmg', label: 'signed-dmg' },
      { id: 'dev-server', label: 'dev-server' }
    ],
    worktrees: [
      { id: 'main', label: 'main' },
      { id: 'sidebar', label: 'wt/sidebar-craft' },
      { id: 'remote', label: 'wt/remote-holder' }
    ],
    remote: [
      { id: 'probe', label: 'probe' },
      { id: 'holder', label: 'holder.toml' },
      { id: 'attach', label: 'attach' }
    ],
    engine: [
      { id: 'hello', label: 'hello' },
      { id: 'record', label: 'session-record' },
      { id: 'status', label: 'status' }
    ],
    orchestration: [
      { id: 'spawn', label: 'spawn_agent' },
      { id: 'wait', label: 'wait_for_agent' },
      { id: 'read', label: 'read_output' }
    ]
  };

  const code: Record<string, CodeLine[]> = {
    'worktrees:main': [
      { text: '# Shared checkout. Do not let two writers land here.', comment: true },
      { text: 'project: zeus' },
      { text: 'path: ~/Desktop/zeus/zeus' },
      { text: 'branch: main' },
      { text: 'session: null' }
    ],
    'worktrees:sidebar': [
      { text: '# Isolated checkout for a concurrent writer.', comment: true },
      { text: 'project: zeus' },
      { text: 'path: .zeus/worktrees/sidebar-craft' },
      { text: 'branch: sidebar-craft' },
      { text: 'session: polish-sidebar' },
      { text: 'agent: codex' },
      { text: 'parent: null' }
    ],
    'worktrees:remote': [
      { text: '# Local worktree. Remote sessions do not share this checkout.', comment: true },
      { text: 'project: zeus' },
      { text: 'path: .zeus/worktrees/remote-holder' },
      { text: 'session: remote-pty' },
      { text: 'agent: claude-code' },
      { text: 'host: gpu-box' }
    ],
    'remote:probe': [
      { text: 'ssh -T gpu-box -- zeus-remote probe' },
      { text: 'os: linux' },
      { text: 'arch: aarch64' },
      { text: 'protocol: remote_pty/1' },
      { text: 'persistence: native-detach' }
    ],
    'remote:holder': [
      { text: '# One Holder process and Unix socket per Session.', comment: true },
      { text: 'session: remote-pty' },
      { text: 'helper: zeus-remote' },
      { text: 'build_id: 0.3.0+a1b2' },
      { text: 'owns:' },
      { text: '  - pty' },
      { text: '  - agent process tree' },
      { text: '  - terminal grid' },
      { text: '  - controller lease' }
    ],
    'remote:attach': [
      { text: '# SSH is the byte transport. It does not own the PTY.', comment: true },
      { text: 'channel: ssh -T' },
      { text: 'attach: full snapshot' },
      { text: 'controller_epoch: 4' },
      { text: 'input_to_pty: direct' },
      { text: 'tmux: false' }
    ],
    'engine:hello': [
      { text: 'zeus-app  →  Hello  →  zeus-engine' },
      { text: 'identity: rust-engine' },
      { text: 'protocol: current' },
      { text: 'fail_closed: missing | old | unknown' }
    ],
    'engine:record': [
      { text: 'SessionRecord {' },
      { text: '  id: sess_sidebar' },
      { text: '  kind: codex' },
      { text: '  cwd: .zeus/worktrees/sidebar-craft' },
      { text: '  host: local' },
      { text: '  pty: engine-owned' },
      { text: '}' }
    ],
    'engine:status': [
      { text: '# Reduced from the grid the agent actually renders.', comment: true },
      { text: 'working      agent is doing something' },
      { text: 'needs input  blocked on you' },
      { text: 'done         finished, not yet opened' },
      { text: 'idle         quiet, already seen' },
      { text: 'ended        process exited' }
    ],
    'orchestration:spawn': [
      { text: 'spawn_agent({' },
      { text: '  kind: "codex",' },
      { text: '  name: "sidebar-craft",' },
      { text: '  worktree: true,' },
      { text: '  cwd: "~/Desktop/zeus/zeus",' },
      { text: '  prompt: "Polish the left sidebar hierarchy"' },
      { text: '})' }
    ],
    'orchestration:wait': [
      { text: 'wait_for_agent({' },
      { text: '  id: "sess_sidebar",' },
      { text: '  until: "done" | "needsInput" | "exited"' },
      { text: '})' },
      { text: '' },
      { text: '# The child is a real Zeus session: sidebar row, diff, PTY.', comment: true }
    ],
    'orchestration:read': [
      { text: 'read_output(id: "sess_sidebar")' },
      { text: 'send_prompt(id, "allow cargo fmt")' },
      { text: 'release_agent(id)' },
      { text: '' },
      { text: '# Lead agents call these on the zeus MCP server.', comment: true }
    ]
  };

  const footnotes: Record<Concept, string> = {
    sessions: 'Each row is a real PTY. Status is reduced from the screen the agent renders.',
    worktrees: 'Concurrent writers get isolated Git checkouts. A worktree is not a sandbox.',
    remote: 'A Helper on the host owns the PTY. Missing transport fails closed. No tmux.',
    engine: 'The desktop is a client. Closing the window does not kill the process tree.',
    orchestration: 'spawn_agent is the only spawn that becomes a sidebar session.'
  };

  let concept = $state<Concept>('sessions');
  let fileId = $state('sidebar');
  let sessionId = $state('sidebar');

  const fileList = $derived(files[concept]);
  const activeSession = $derived(sessions.find((session) => session.id === sessionId) ?? sessions[1]);
  const codeKey = $derived(`${concept}:${fileId}`);
  const activeLines = $derived(code[codeKey] ?? []);
  const footnote = $derived(footnotes[concept]);

  function selectConcept(next: Concept) {
    concept = next;
    fileId = files[next][0].id;
    if (next === 'sessions') sessionId = fileId;
  }

  function selectFile(id: string) {
    fileId = id;
    if (concept === 'sessions') sessionId = id;
  }

  function selectSession(id: string) {
    sessionId = id;
    fileId = id;
  }
</script>

<div class="panel" role="region" aria-label="Zeus system demonstration">
  <div class="panel__tabs">
    {#each concepts as item}
      <button
        type="button"
        class="tab"
        aria-pressed={concept === item.id}
        onclick={() => selectConcept(item.id)}>{item.label}</button
      >
    {/each}
  </div>

  <div class="panel__meta">
    <span class="panel__meta-label">
      {#if concept === 'sessions'}
        working · needs input · done
      {:else if concept === 'worktrees'}
        isolated checkouts · one writer each
      {:else if concept === 'remote'}
        ssh -T · remote_pty · one Holder per session
      {:else if concept === 'engine'}
        zeus-app → zeus-engine · fail closed
      {:else}
        MCP · spawn_agent · wait_for_agent
      {/if}
    </span>
    <span class="chip">{concept === 'remote' ? 'gpu-box' : 'This Mac'}</span>
  </div>

  <div class="panel__files">
    {#each fileList as file}
      <button
        type="button"
        class="file-tab"
        aria-pressed={fileId === file.id}
        onclick={() => selectFile(file.id)}>{file.label}</button
      >
    {/each}
  </div>

  {#if concept === 'sessions'}
    <div class="workbench">
      <aside class="workbench__side" aria-label="Sessions">
        <div class="workbench__project">
          <span>Zeus</span>
          <span>This Mac</span>
        </div>
        {#each sessions as session}
          <button
            type="button"
            class="session-row"
            class:session-row--child={session.child}
            aria-current={session.id === sessionId}
            onclick={() => selectSession(session.id)}
          >
            <span class="status status--{session.status}" aria-hidden="true"></span>
            <span class="session-row__title">{session.title}</span>
            <span class="session-row__kind">{session.kind}</span>
          </button>
        {/each}
      </aside>
      <div class="term">
        <div class="term__bar">
          <span>{activeSession.kind}</span>
          <span aria-hidden="true">·</span>
          <span>{activeSession.host}</span>
          <span aria-hidden="true">·</span>
          <span>{activeSession.cwd}</span>
        </div>
        <div class="term__body">
          {#each activeSession.body as line}
            <div class={line.startsWith('$') || line.startsWith('Needs') ? 'term__prompt' : 'term__muted'}>
              {line || '\u00a0'}
            </div>
          {/each}
          {#if activeSession.prompt}
            <div class="term__prompt" style="margin-top: 16px">
              &gt; {activeSession.prompt}<span class="term__cursor" aria-hidden="true"></span>
            </div>
          {/if}
        </div>
      </div>
    </div>
  {:else}
    <div class="code-surface" aria-label={fileId}>
      <pre class="code-surface__gutter">{activeLines.map((_, i) => i + 1).join('\n')}</pre>
      <pre class="code-surface__lines">{#each activeLines as line}{#if line.comment}<span class="code-surface__comment">{line.text}</span>{:else}{line.text}{/if}{'\n'}{/each}</pre>
    </div>
  {/if}

  <div class="panel__footer">{footnote}</div>
</div>
