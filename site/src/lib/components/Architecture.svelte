<div class="diagram" role="img" aria-labelledby="arch-title arch-desc">
  <p id="arch-title" class="sr-only">Zeus session architecture</p>
  <p id="arch-desc" class="sr-only">
    The desktop app talks to a local Engine. The Engine owns session records, status, and local
    PTYs. Remote sessions go over SSH to a Helper that holds one PTY each.
  </p>
  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1080 420" fill="none">
    <defs>
      <marker
        id="arch-arrow"
        viewBox="0 0 10 10"
        refX="8.5"
        refY="5"
        markerWidth="6"
        markerHeight="6"
        orient="auto"
      >
        <path d="M0 0.6L9 5L0 9.4z" fill="#FFFFFF" />
      </marker>
    </defs>

    <rect class="ap" x="40" y="150" width="200" height="140" rx="14" />
    <text class="at" x="140" y="198" text-anchor="middle">Zeus.app</text>
    <text class="as" x="140" y="222" text-anchor="middle">GPUI workbench</text>
    <rect class="ac" x="68" y="244" width="144" height="28" rx="8" />
    <text class="al" x="140" y="263" text-anchor="middle">attach / input</text>

    <path class="af" d="M240 220 H300" marker-end="url(#arch-arrow)" />
    <text class="as" x="270" y="208" text-anchor="middle">Hello</text>

    <rect class="ap" x="300" y="90" width="280" height="260" rx="14" />
    <text class="at" x="440" y="132" text-anchor="middle">Engine</text>
    <text class="as" x="440" y="154" text-anchor="middle">session identity</text>
    <rect class="ac" x="324" y="178" width="110" height="28" rx="8" />
    <text class="al" x="379" y="197" text-anchor="middle">SessionRecord</text>
    <rect class="ac" x="444" y="178" width="112" height="28" rx="8" />
    <text class="al" x="500" y="197" text-anchor="middle">status</text>
    <rect class="ac" x="324" y="216" width="110" height="28" rx="8" />
    <text class="al" x="379" y="235" text-anchor="middle">manifests</text>
    <rect class="ac" x="444" y="216" width="112" height="28" rx="8" />
    <text class="al" x="500" y="235" text-anchor="middle">bootstrap</text>
    <rect class="ac" x="354" y="268" width="172" height="28" rx="8" />
    <text class="al" x="440" y="287" text-anchor="middle">PTY lifecycle</text>
    <text class="as" x="440" y="326" text-anchor="middle">owns the process tree</text>

    <path class="af" d="M580 160 H650" marker-end="url(#arch-arrow)" />
    <path class="af" d="M580 280 H650" marker-end="url(#arch-arrow)" />

    <rect class="ap" x="650" y="90" width="390" height="120" rx="14" />
    <text class="at" x="845" y="132" text-anchor="middle">Local PTY</text>
    <text class="as" x="845" y="154" text-anchor="middle">Claude · Codex · Grok · shell</text>
    <rect class="ac" x="700" y="170" width="90" height="24" rx="8" />
    <text class="al" x="745" y="187" text-anchor="middle">argv</text>
    <rect class="ac" x="800" y="170" width="90" height="24" rx="8" />
    <text class="al" x="845" y="187" text-anchor="middle">cwd</text>
    <rect class="ac" x="900" y="170" width="90" height="24" rx="8" />
    <text class="al" x="945" y="187" text-anchor="middle">env</text>

    <rect class="ag" x="640" y="236" width="410" height="150" rx="18" />
    <text class="as" x="845" y="258" text-anchor="middle">remote host</text>
    <rect class="ap" x="660" y="270" width="370" height="96" rx="14" />
    <text class="at" x="845" y="308" text-anchor="middle">zeus-remote Holder</text>
    <text class="as" x="845" y="332" text-anchor="middle">one socket · one PTY · controller lease</text>

    <text class="as" x="615" y="268" text-anchor="middle">ssh -T</text>
  </svg>
</div>
