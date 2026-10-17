// Gemini ASCII mark: Copyright 2025 Google LLC, Apache-2.0.
// Source: google-gemini/gemini-cli, packages/cli/src/ui/components/AsciiArt.ts
const geminiAscii = `   █████████  ██████████ ██████   ██████ █████ ██████   █████ █████
  ███░░░░░███░░███░░░░░█░░██████ ██████ ░░███ ░░██████ ░░███ ░░███
 ███     ░░░  ░███  █ ░  ░███░█████░███  ░███  ░███░███ ░███  ░███
░███          ░██████    ░███░░███ ░███  ░███  ░███░░███░███  ░███
░███    █████ ░███░░█    ░███ ░░░  ░███  ░███  ░███ ░░██████  ░███
░░███  ░░███  ░███ ░   █ ░███      ░███  ░███  ░███  ░░█████  ░███
 ░░█████████  ██████████ █████     █████ █████ █████  ░░█████ █████
  ░░░░░░░░░  ░░░░░░░░░░ ░░░░░     ░░░░░ ░░░░░ ░░░░░    ░░░░░ ░░░░░`;

// Terminal-specific presentation for the website's illustrative sessions.
// UI references are listed in README.md; none of these views runs an agent.
const agentPreviews = (() => {
  const esc = value => value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  const mark = name => `<span class="agent-logo ${name}" data-agent="${name}" aria-hidden="true"></span>`;
  const tool = (name, file, detail) => `<div class="claude-tool"><span class="claude-bullet">●</span><div><strong>${name}</strong><span>(${file})</span><small>└ ${detail}</small></div></div>`;
  const changes = (file, lines, stat = '') => `<div class="diff-file"><span class="icon" data-icon="code"></span>${file}${stat ? `<span class="diff-stat">${stat}</span>` : ''}</div><div class="diff-lines">${lines.map((line, i) => { const kind = line[0] === '+' ? 'added' : line[0] === '-' ? 'removed' : ''; return `<div class="${kind}"><span>${kind === 'added' ? '+' : kind === 'removed' ? '−' : i + 1}</span><code>${esc(kind ? line.slice(1) : line)}</code></div>`; }).join('')}</div>`;
  const command = (symbol, placeholder) => `<span class="composer-prefix">${symbol}</span><span>${placeholder}</span><span class="terminal-cursor" aria-hidden="true"></span>`;
  return {
    swarm: {
      // A long-running session, scrolled to its end: earlier work above, the swarm at the bottom.
      // app.js opens it mid-run (helpers working) and finishes it on a timeline.
      render(chat) {
        const child = (n, title, agent, time) => `<div class="swarm-child" data-child="${n}"><span class="swarm-state done">✓</span><span>${title}</span>${mark(agent)}<small>${time}</small></div>`;
        const answer = (step, text) => `<div class="claude-answer"${step ? ` data-step="${step}"` : ''}><span class="claude-bullet">●</span><div><p>${text}</p></div></div>`;
        const user = text => `<div class="claude-user">❯ ${text}</div>`;
        const worked = text => `<div class="claude-worked">✻ ${text}</div>`;
        return {
          tail: true,
          html: `${user('What’s still open for the 0.9 milestone?')}${tool('Bash', 'gh issue list --milestone 0.9 --state open', '7 open issues')}${tool('Read', 'ROADMAP.md', 'Read 64 lines')}${answer('', 'Seven left. Four are polish; three block the release: the Linux updater, Markdown export for notes, and the Wayland clipboard.')}${user('Fix the three blockers. I’ll review.')}${tool('Update', 'src/updater/linux.rs', 'Updated with 18 additions and 6 removals')}${tool('Update', 'src/notes/export.rs', 'Updated with 9 additions and 2 removals')}${tool('Update', 'src/platform/wayland.rs', 'Updated with 12 additions and 4 removals')}${tool('Bash', 'cargo test --workspace', '1,642 passed, 0 failed, 45 ignored')}${answer('', 'All three blockers are fixed and covered by tests. The diff is in Changes.')}${worked('Worked for 14m 32s')}<div class="claude-user" data-step="prompt">❯ ${chat.prompt}</div>${tool('Bash', 'git log v0.8.0..HEAD --oneline', '212 commits since 0.8.0')}${answer('plan', 'Three independent pieces. I’ll give each its own agent and worktree.')}<div class="claude-tool" data-step="spawn"><span class="claude-bullet">●</span><div><strong>Taskmaster</strong><span> - spawn_agents (3 agents)</span><div class="swarm-list">${child(1, 'Write the release notes', 'codex', '2m 10s')}${child(2, 'Verify the Linux build', 'claude', '6m 48s')}${child(3, 'Test the updater', 'gemini', '9m 02s')}</div></div></div>${answer('notes', 'Release notes are in. Reviewing the diff while the other two finish.')}${answer('done', 'All three are done: notes written, Linux build verified, updater tested. Ready to tag 0.9.')}`,
          composer: command('❯', 'Ask a follow-up'),
          footer: '<span class="swarm-footer">3 agents finished</span><span>Opus</span>',
          changes: `<div class="changes-summary"><span>6 files changed</span><span class="added-text">+71</span><span class="removed-text">−14</span></div>`
            + changes('src/updater/linux.rs', ['fn swap_bundle(new: &Path) -> Result<()> {', '-    fs::rename(new, &current)?;', '+    let staged = current.with_extension("staged");', '+    fs::rename(new, &staged)?;', '+    verify_bundle(&staged)?;', '+    fs::rename(&staged, &current)?;', '     Ok(())', '}'], '+18 −6')
            + changes('src/notes/export.rs', ['pub fn to_markdown(note: &Note) -> String {', '-    note.body.clone()', '+    let mut out = format!("# {}\\n\\n", note.title);', '+    out.push_str(&note.body);', '+    out', '}'], '+9 −2')
            + changes('src/platform/wayland.rs', ['match clipboard.read() {', '-    Err(_) => None,', '+    Err(e) if e.is_transient() => retry(clipboard),', '+    Err(_) => None,', '     Ok(text) => Some(text),', '}'], '+12 −4')
            + changes('RELEASE_NOTES.md', ['+## 0.9', '+', '+- Notes, and to-dos that start agents', '+- Agents can start other agents', '+- Faster session restore', '+- Linux: Wayland clipboard fixes'], '+24')
            + changes('taskmaster/Cargo.toml', ['[workspace.package]', '-version = "0.8.4"', '+version = "0.9.0"', 'edition = "2024"'], '+1 −1')
        };
      }
    },
    codex: {
      render(chat) {
        return {
          html: `<div class="codex-welcome"><strong><span>›_</span> OpenAI Codex</strong><div><span>model:</span> gpt-6 <small>/model to change</small></div><div><span>directory:</span> ~/fun/taskmaster</div></div><div class="codex-user">› ${chat.prompt}</div><p class="cli-response">I’ll match the app’s layout and keep the terminal opaque.</p><div class="codex-explored"><strong>Explored</strong><div>└ Read <span>index.html, style.css</span></div></div><div class="codex-explored"><strong>Edited</strong><div>└ <span>website/style.css</span> <em>+5</em></div></div><div class="cli-divider">Worked for 42s</div><div class="cli-summary"><p>Updated the preview:</p><p>— Native window proportions<br>— Translucent sidebars and toolbar<br>— Rosé Pine shader background</p><button class="result-link" data-open="links">Open preview ↗</button></div>`,
          composer: command('›', 'Ask a question or describe a task'),
          footer: '<span>gpt-6</span><span>~/fun/taskmaster</span>',
          changes: changes('website/style.css', [':root {','+  --background: #05070c;','+  --accent: #3b82f6;','}', '', '.terminal-body {', '+  background: #05070c;', '}', '', '.app-window {', '+  aspect-ratio: 16 / 10;', '+  backdrop-filter: blur(16px);', '}'])
        };
      }
    },
    claude: {
      render(chat, answer) {
        const reply = answer
          ? `${tool('Write', 'this-week.md', 'Wrote weekly plan')}<div class="claude-answer"><span class="claude-bullet">●</span><div><strong>Plan saved.</strong><p>1. Finish the first draft<br>2. Collect feedback<br>3. Refine the design</p>${answer === 'full' ? '<p>Monday: first draft<br>Wednesday: feedback<br>Friday: revisions</p>' : ''}</div></div>`
          : `${tool('Read', 'notes.md', 'Read 24 lines')}<div class="claude-answer"><span class="claude-bullet">●</span><div><p>How much detail should the plan include?</p><div class="claude-choices"><button data-answer="short"><span>1.</span> Summary</button><button data-answer="full"><span>2.</span> Detailed plan</button></div></div></div>`;
        return {
          html: `<div class="claude-welcome"><pre class="claude-mascot" aria-hidden="true"> ▐▛███▜▌\n▝▜█████▛▘\n  ▘▘ ▝▝</pre><div><strong>Claude Code</strong><span>Sonnet</span><small>~/fun/taskmaster</small></div></div><div class="claude-user">❯ ${chat.prompt}</div>${reply}`,
          composer: command('❯', 'Try "summarize this project"'),
          footer: '<span>? for shortcuts</span><span>Sonnet</span>',
          changes: changes('this-week.md', ['+# This week', '+', '+- Finish the first draft', '+- Collect feedback', '+- Refine the design'])
        };
      }
    },
    cursor: {
      render(chat) {
        return {
          html: `<div class="cursor-welcome">${mark('cursor')}<div><strong>Cursor Agent</strong><small>~/fun/taskmaster <span>main</span></small></div></div><div class="cursor-user"><span>→</span>${chat.prompt}</div><p class="cursor-thinking">I’ll check the focus handling and key bindings.</p><div class="cursor-tool"><span>Read</span><code>navigation.rs</code><small>128 lines</small></div><div class="cursor-tool"><span>Edit</span><code>navigation.rs</code><small class="cursor-gain">+3</small></div><div class="cursor-patch"><span>+ Key::Up =&gt; select_previous(),</span><span>+ Key::Down =&gt; select_next(),</span><span>+ Key::Escape =&gt; close_menu(),</span></div><p class="cursor-result">Arrow keys move the selection. Escape closes the menu and restores focus.</p><button class="result-link" data-open="command">Open commands ↗</button>`,
          composer: command('→', 'Add a follow-up'),
          footer: '<span>Auto</span><span>/ commands &nbsp; @ files &nbsp; ! shell</span>',
          changes: changes('navigation.rs', ['match key {', '+  Key::Up => select_previous(),', '+  Key::Down => select_next(),', '+  Key::Escape => close_menu(),', '  _ => {}', '}'])
        };
      }
    },
    gemini: {
      render(chat) {
        return {
          html: `<pre class="gemini-ascii" aria-label="Gemini CLI">${esc(geminiAscii)}</pre><div class="gemini-user"><span>›</span>${chat.prompt}</div><div class="gemini-tool"><div><span>✓</span><strong>WriteFile</strong><code>src/Map.tsx</code></div><p>Created map with saved places and notes</p></div><div class="gemini-tool"><div><span>✓</span><strong>Shell</strong><code>npm run build</code></div><p>Build completed successfully</p></div><div class="gemini-answer"><span>✦</span><p>The map is ready. Add a place to save it, or select a pin to edit its note.</p></div>`,
          composer: command('>', 'Type your message or @path/to/file'),
          footer: '<span>~/travel-map</span><span>Auto</span>',
          changes: changes('src/Map.tsx', ['+export function Map() {', '+  return (', '+    <MapView>', '+      <SavedPlaces />', '+    </MapView>', '+  );', '+}'])
        };
      }
    }
  };
})();

globalThis.agentPreviews = agentPreviews;
