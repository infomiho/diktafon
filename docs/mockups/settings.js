//! Settings mock: the window shell (sidebar + pane slot) and the interactive
//! History pane. Panes stay light-DOM HTML composed from the shared `dk-*`
//! components; only History owns enough state (playing track, delete-all
//! confirm, copy flash) to be a component itself.

import { LitElement, html, css } from 'https://esm.sh/lit@3.3.3';
import './dk.js?v=5';

const SECTIONS = [
  { id: 'general', label: 'General', icon: '⚙', title: 'General' },
  { id: 'models', label: 'Models', icon: '◈', title: 'Models' },
  { id: 'history', label: 'History', icon: '◷', title: 'History' },
  { id: 'advanced', label: 'Advanced', icon: '▣', title: 'Advanced' },
];

/// A settings window specimen: sidebar chrome plus a slotted pane, so the
/// five window copies share one sidebar instead of five.
export class DkSettingsWindow extends LitElement {
  static properties = { section: { type: String } };
  static styles = css`
    :host {
      width: 720px; height: 500px; background: var(--background); border-radius: 12px;
      display: flex; overflow: hidden; box-shadow: 0 24px 60px rgba(0, 0, 0, 0.6);
      position: relative;
    }
    .sidebar {
      width: 200px; flex-shrink: 0; background: var(--surface-sunken);
      border-right: 1px solid var(--outline); padding: 48px 12px 12px;
      display: flex; flex-direction: column; gap: 6px; position: relative;
    }
    .brand-row { display: flex; align-items: center; gap: 9px; padding: 4px 12px 14px; }
    .brand-row img { width: 32px; height: 18px; }
    .brand-row span {
      font-family: 'Chakra Petch', sans-serif; font-size: 16px; font-weight: 600;
      letter-spacing: 0.02em;
    }
    .pane {
      flex: 1; min-width: 0; padding: 32px; display: flex; flex-direction: column;
      overflow-y: auto; scrollbar-width: thin; scrollbar-color: var(--surface-raised) transparent;
    }
    h1 {
      font-family: var(--font-display); font-size: 24px; font-weight: 600;
      line-height: 1.2; letter-spacing: 0.01em; margin: 0;
    }
    .content { margin-top: 32px; display: flex; flex-direction: column; gap: 32px; }
  `;
  render() {
    const title = SECTIONS.find((s) => s.id === this.section)?.title ?? '';
    return html`
      <div class="sidebar">
        <dk-traffic></dk-traffic>
        <div class="brand-row">
          <img src="../../assets/diktafon-mark-flat.svg" width="32" height="18" alt="" />
          <span>diktafon</span>
        </div>
        ${SECTIONS.map(
          (s) => html`<dk-nav-item icon=${s.icon} ?active=${s.id === this.section}>${s.label}</dk-nav-item>`,
        )}
      </div>
      <div class="pane">
        <h1>${title}</h1>
        <div class="content"><slot></slot></div>
      </div>
    `;
  }
}
customElements.define('dk-settings-window', DkSettingsWindow);

/// Demo dictations: `audio` true has a clip, `'missing'` had one, false never did.
const HISTORY = [
  {
    day: 'Today',
    rows: [
      {
        time: '10:37',
        text: 'Where we kind of have the jobs refactor in PR 1. Then we do this: isolated groups refactor in PR 2, and then we add the managed processes and anodamone removal in PR 3. How does that sound? How would that look like?',
        audio: true,
        rerun: {
          caption: 'Canary 1B Flash + S1-mini',
          text: 'Where we kind of have the jobs refactor in PR 1. Then we do this: isolated groups refactor in PR 2, and then we add the managed processes and a daemon removal in PR 3. How does that sound? How would that look like?',
        },
      },
      {
        time: '09:31',
        text: 'Compare that to the dark background we have here.',
        audio: true,
        working: true,
      },
      {
        time: '09:28',
        text: "It doesn't really look good when there is a light background behind it. This leads me to believe that maybe we shouldn't be doing so much transparency.",
        audio: false,
      },
    ],
  },
  {
    day: 'Yesterday',
    rows: [
      {
        time: '14:02',
        text: "Penguin Enterprises' flagship Master 3000 utilizes a proprietary cryo-resistant polymer blend rated for continuous operation at temperatures down to minus 60 degrees.",
        audio: 'missing',
      },
    ],
  },
];

/// The History pane: day-grouped rows with play, copy, and delete. Delete
/// removes the whole dictation, transcript and clip together.
export class DkHistory extends LitElement {
  static properties = {
    _days: { state: true },
    _playing: { state: true },
    _copied: { state: true },
    _notice: { state: true },
  };
  static styles = css`
    .search {
      height: 40px; border: 1px solid var(--outline); border-radius: 6px;
      display: flex; align-items: center; padding: 0 12px; background: var(--surface);
      font-size: 15px; color: var(--faint); margin-bottom: 20px;
    }
    .hlist { display: flex; flex-direction: column; gap: 6px; }
    .hday { font-size: 13px; font-weight: 500; color: var(--muted); margin: 20px 0 4px; }
    .hday:first-child { margin-top: 0; }
    .hrow {
      display: flex; align-items: flex-start; gap: 14px;
      padding: 13px 14px; margin: 0 -14px; border-radius: 8px;
    }
    .hrow:hover { background: var(--surface-raised); }
    .htime {
      width: 40px; flex-shrink: 0; font-size: 13px; color: var(--faint);
      font-variant-numeric: tabular-nums; padding-top: 1px;
    }
    .hmain { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 8px; }
    .htext {
      font-size: 15px; line-height: 1.5; color: var(--on-surface);
      display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;
    }
    .hactions { display: flex; gap: 2px; }
    .hbtn:active { transform: scale(0.96); }
    .rcard {
      margin-top: 8px; padding: 12px 14px; border-radius: 8px;
      background: var(--surface); border: 1px solid var(--outline);
      display: flex; flex-direction: column; gap: 8px;
    }
    .ractions { display: flex; gap: 2px; }
    .rfoot { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
    .rtext { font-size: 15px; line-height: 1.5; color: var(--on-surface); }
    .chg {
      background: color-mix(in srgb, var(--status-ok) 22%, transparent);
      border-radius: 3px; padding: 0 1px;
    }
    .rcap { font-size: 13px; color: var(--faint); }
    .skel { display: flex; flex-direction: column; gap: 8px; margin-top: 8px; }
    .skel i {
      display: block; height: 14px; border-radius: 4px;
      background: var(--surface-raised);
      animation: skel-pulse 1200ms ease-in-out infinite;
    }
    .skel i:last-child { width: 55%; }
    @keyframes skel-pulse {
      0%, 100% { opacity: 1; }
      50% { opacity: 0.45; }
    }
    @media (prefers-reduced-motion: reduce) {
      .skel i { animation: none; }
    }
    .hnotice { font-size: 13px; color: var(--on-surface-muted); padding: 12px 2px 0; }
  `;
  constructor() {
    super();
    this._days = structuredClone(HISTORY);
    this._playing = null;
    this._copied = null;
    this._notice = null;
  }
  _rowId(day, ix) {
    return `${day}:${ix}`;
  }
  _togglePlay(day, ix) {
    const id = this._rowId(day, ix);
    this._playing = this._playing === id ? null : id;
  }
  _flashNotice(text) {
    this._notice = text;
    clearTimeout(this._noticeTimer);
    this._noticeTimer = setTimeout(() => {
      this._notice = null;
      this.requestUpdate();
    }, 2500);
  }
  _deleteEntry(day, ix) {
    // Delete means the whole entry: transcript and clip go together. Row
    // indices shift underneath playback, so any delete stops the player.
    const group = this._days.find((g) => g.day === day);
    if (!group || !group.rows[ix]) {
      this._flashNotice('Audio file already gone.');
      return;
    }
    group.rows.splice(ix, 1);
    this._days = this._days.filter((g) => g.rows.length > 0);
    this._playing = null;
    this._flashNotice('Dictation deleted.');
    this.requestUpdate();
  }
  /// Word-level diff of the rerun against the original: returns tokens with
  /// a flag for rerun-side words the original does not have, so differences
  /// pop without reading both texts twice.
  _diffTokens(orig, rerun) {
    const a = orig.split(/\s+/);
    const b = rerun.split(/\s+/);
    const lcs = Array.from({ length: a.length + 1 }, () => new Array(b.length + 1).fill(0));
    for (let i = a.length - 1; i >= 0; i--) {
      for (let j = b.length - 1; j >= 0; j--) {
        lcs[i][j] =
          a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
      }
    }
    const out = [];
    let i = 0;
    let j = 0;
    while (i < a.length && j < b.length) {
      if (a[i] === b[j]) {
        out.push({ text: b[j], changed: false });
        i++;
        j++;
      } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
        i++;
      } else {
        out.push({ text: b[j], changed: true });
        j++;
      }
    }
    while (j < b.length) {
      out.push({ text: b[j], changed: true });
      j++;
    }
    return out;
  }
  _acceptRerun(day, ix) {
    const group = this._days.find((g) => g.day === day);
    const target = group && group.rows[ix];
    if (target && target.rerun) {
      try {
        navigator.clipboard.writeText(target.rerun.text);
      } catch {
        // Clipboard needs a gesture or secure context; the notice covers it.
      }
      delete target.rerun;
      this._flashNotice('Rerun accepted. Copied to clipboard.');
      this.requestUpdate();
    }
  }
  _rejectRerun(day, ix) {
    const group = this._days.find((g) => g.day === day);
    const target = group && group.rows[ix];
    if (target && target.rerun) {
      delete target.rerun;
      this.requestUpdate();
    }
  }
  _rerunCard(row, day, ix) {
    const tokens = this._diffTokens(row.text, row.rerun.text);
    return html`<div class="rcard">
      <div class="rtext">${tokens.map(
        (t) => (t.changed ? html`<span class="chg">${t.text}</span> ` : html`${t.text} `),
      )}</div>
      <div class="rfoot">
        <div class="rcap">${row.rerun.caption}</div>
        <div class="ractions">
          <dk-icon-btn icon="accept" label="Accept rerun" accent @click=${() =>
            this._acceptRerun(day, ix)}></dk-icon-btn>
          <dk-icon-btn
            icon="reject"
            label="Reject rerun"
            danger
            @click=${() => this._rejectRerun(day, ix)}
          ></dk-icon-btn>
        </div>
      </div>
    </div>`;
  }
  _copy(day, ix) {
    const id = this._rowId(day, ix);
    this._copied = id;
    clearTimeout(this._copyTimer);
    this._copyTimer = setTimeout(() => {
      if (this._copied === id) {
        this._copied = null;
        this.requestUpdate();
      }
    }, 1500);
  }
  _rowButtons(row, day, ix) {
    const id = this._rowId(day, ix);
    const playing = this._playing === id;
    const buttons = [];
    if (row.audio === true) {
      buttons.push(html`<dk-icon-btn
        icon=${playing ? 'pause' : 'play'}
        label=${playing ? 'Pause playback' : 'Play recording'}
        ?active=${playing}
        @click=${() => this._togglePlay(day, ix)}
      ></dk-icon-btn>`);
      // Placeholder for t70.3: reruns the clip through the selected models.
      buttons.push(html`<dk-icon-btn
        icon="retranscribe"
        label="Retranscribe with current models"
      ></dk-icon-btn>`);
    }
    buttons.push(html`<dk-icon-btn
      icon="copy"
      label="Copy dictation"
      ?active=${this._copied === id}
      @click=${() => this._copy(day, ix)}
    ></dk-icon-btn>`);
    buttons.push(html`<dk-icon-btn
      icon="trash"
      label="Delete dictation"
      danger
      @click=${() => this._deleteEntry(day, ix)}
    ></dk-icon-btn>`);
    return buttons;
  }
  render() {
    return html`
      <div class="search">Search 108 dictations</div>
      <div class="hlist">
        ${this._days.map(
          (group) => html`
            <div class="hday">${group.day}</div>
            ${group.rows.map(
              (row, ix) => html`
                <div class="hrow">
                  <div class="htime">${row.time}</div>
                  <div class="hmain">
                    <div class="htext">${row.text}</div>
                    <div class="hactions">${this._rowButtons(row, group.day, ix)}</div>
                    ${row.working
                      ? html`<div class="rcard">
                          <div class="rcap">Retranscribing…</div>
                          <div class="skel"><i></i><i></i></div>
                        </div>`
                      : ''}
                    ${row.rerun && !row.working ? this._rerunCard(row, group.day, ix) : ''}
                  </div>
                </div>
              `,
            )}
          `,
        )}
      </div>
      ${this._notice ? html`<div class="hnotice">${this._notice}</div>` : ''}
    `;
  }
}
customElements.define('dk-history', DkHistory);
