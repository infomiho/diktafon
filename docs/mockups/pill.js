//! Pill mock: the floating pill as a component. `phase` and `reduceMotion` are
//! attributes; `showPhase()` and `endSession()` drive it like the app's phase
//! events do. The meter ballistics, aurora, and endings read live state every
//! animation frame, exactly as the surgery-free prototype did.

import { LitElement, html, css } from 'https://esm.sh/lit@3.3.3';
import './dk.js?v=2';

const COLS = 5;
const ROWS = 3;

// Phase color targets; the live color lerps toward them (no hard cuts).
const COLORS = {
  recording: [255, 59, 77],
  transcribing: [255, 255, 255],
  polishing: [206, 92, 255],
  cancel: [142, 144, 190],
  error: [255, 59, 77],
  bloom: [255, 255, 255],
};

// The app's meter: 16 log-spaced bands over 400-8000Hz (sibilance needs the
// top octave), averaged into the 5 columns mel-ish.
const BAND_COUNT = 16;
const FREQ_MIN = 400;
const FREQ_MAX = 8000;
const TILT_DB_PER_OCT = 6.5;
const COLUMN_GROUPS = [
  [0, 4],
  [4, 7],
  [7, 10],
  [10, 13],
  [13, 16],
];

export class DkPill extends LitElement {
  static properties = {
    phase: { type: String, reflect: true },
    reduceMotion: { type: Boolean, attribute: 'reduce-motion', reflect: true },
  };
  static styles = css`
    :host { display: block; }
    .pill {
      width: 172px; height: 38px;
      border-radius: 19px;
      background: color-mix(in srgb, var(--surface) 91%, transparent);
      border: 1px solid var(--outline);
      display: flex;
      align-items: center;
      padding: 0 14px;
      position: relative;
      overflow: hidden;
      /* The pill shows on every dictation, so entry is quick: rise + fade
         with a strong ease-out. Exit sinks along the same path, faster. */
      transition: transform 260ms cubic-bezier(0.23, 1, 0.32, 1), opacity 200ms ease-out, width 260ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .pill.hidden {
      transform: translateY(9px);
      opacity: 0;
      transition: transform 200ms ease, opacity 160ms ease;
    }
    :host([reduce-motion]) .pill, :host([reduce-motion]) .pill.hidden {
      transition: opacity 200ms ease; transform: none;
    }
    .aurora { position: absolute; inset: 0; border-radius: 19px; overflow: hidden; pointer-events: none; }
    .aurora i { position: absolute; top: 15px; width: 8px; height: 8px; border-radius: 50%; }
    .content {
      display: flex; align-items: center; gap: 11px; flex: 1; z-index: 1;
      transition: opacity 120ms ease-out;
    }
    .pill.fading .content { opacity: 0; }
    .label {
      font-size: 13px; color: rgba(241, 242, 255, 0.96); flex: 1;
      white-space: nowrap; overflow: hidden;
      transition: opacity 180ms ease-out;
    }
    .label.status { text-align: right; color: rgba(170, 172, 214, 0.55); }
    .time {
      font-size: 11px; color: rgba(170, 172, 214, 0.55);
      font-variant-numeric: tabular-nums;
    }
    .grille { display: grid; grid-template-columns: repeat(5, auto); gap: 3px; flex-shrink: 0; }
    .grille i { width: 4.5px; height: 4.5px; border-radius: 50%; }
    .measurer {
      position: absolute; visibility: hidden; white-space: nowrap;
      font-size: 13px; font-family: system-ui, sans-serif;
    }
  `;
  constructor() {
    super();
    this.phase = 'hidden';
    this.reduceMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;
    this.beat = null;
    this.beatSince = 0;
    this.phaseSince = performance.now();
    this.smooth = new Float32Array(COLS * ROWS);
    this.liveColor = [...COLORS.recording];
    // Global auto-sensitivity (cava's "sens"): one gain over all columns,
    // stepped down fast when any column clips and crept up slowly otherwise.
    this.sens = 1;
    this.audioCtx = null;
    this.analyser = null;
    this.freqData = null;
    this._visual = [];
    this._raf = 0;
    this._t0 = performance.now();
    this._lastText = null;
  }
  connectedCallback() {
    super.connectedCallback();
    const frame = (now) => {
      this._frame(now);
      this._raf = requestAnimationFrame(frame);
    };
    this._raf = requestAnimationFrame(frame);
  }
  disconnectedCallback() {
    super.disconnectedCallback();
    cancelAnimationFrame(this._raf);
    this._visual.forEach(clearTimeout);
    this._visual = [];
  }
  render() {
    return html`
      <div class="pill hidden">
        <div class="aurora"></div>
        <div class="content">
          <div class="grille"></div>
          <div class="label"></div>
          <div class="time"></div>
        </div>
        <span class="measurer"></span>
      </div>
    `;
  }
  firstUpdated() {
    const root = this.shadowRoot;
    this._pill = root.querySelector('.pill');
    this._aurora = root.querySelector('.aurora');
    this._grille = root.querySelector('.grille');
    this._label = root.querySelector('.label');
    this._time = root.querySelector('.time');
    this._measurer = root.querySelector('.measurer');
    for (let i = 0; i < COLS * ROWS; i++) this._grille.appendChild(document.createElement('i'));
    this._dots = [...this._grille.querySelectorAll('i')];
    ['#FF5A36', '#FF3B4D', '#E0459E'].forEach((color, i) => {
      const blob = document.createElement('i');
      blob.style.left = `${30 + i * 55}px`;
      blob.dataset.seed = i * 2.1;
      blob.dataset.color = color;
      this._aurora.appendChild(blob);
    });
    this._at(400, () => this.showPhase('recording'));
  }
  _at(ms, fn) {
    this._visual.push(setTimeout(fn, ms));
  }
  showPhase(p) {
    this.phase = p;
    this.beat = null;
    this.phaseSince = performance.now();
    this._visual.forEach(clearTimeout);
    this._visual = [];
    if (!this._pill) return;
    this._pill.classList.toggle('hidden', p === 'hidden');
    this._pill.classList.remove('fading');
    this.dispatchEvent(new CustomEvent('dk-phase', { detail: p, bubbles: true, composed: true }));
  }
  endSession(kind) {
    this.beat = kind;
    this.beatSince = performance.now();
    // The beat plays on the grille (deliberate), then the contents fade in
    // place and the chip sinks out along the entry path (snappy).
    const hold = kind === 'error' ? 2400 : kind === 'bloom' ? 420 : 160;
    this._at(hold, () => this._pill.classList.add('fading'));
    this._at(hold + 130, () => this._pill.classList.add('hidden'));
    this._at(hold + 480, () => {
      this.showPhase('hidden');
      this._pill.classList.remove('fading');
    });
    this.dispatchEvent(new CustomEvent('dk-beat', { detail: kind, bubbles: true, composed: true }));
  }
  async useMic() {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    this.audioCtx = new AudioContext();
    const src = this.audioCtx.createMediaStreamSource(stream);
    this.analyser = this.audioCtx.createAnalyser();
    this.analyser.fftSize = 256;
    this.analyser.smoothingTimeConstant = 0.7;
    src.connect(this.analyser);
    this.freqData = new Uint8Array(this.analyser.frequencyBinCount);
  }
  _meterBands(levels) {
    const SENS_DOWN = 0.98;
    const SENS_UP = 1.0005;
    // Monstercat-style neighbor spread (from cava): a loud column radiates a
    // skirt into its neighbors, so speech reads as a coherent shape instead
    // of isolated spikes.
    const SPREAD = 2.25;
    let clipped = false;
    const out = levels.map((v) => {
      const scaled = v * this.sens;
      if (scaled > 1) clipped = true;
      return Math.min(1, scaled);
    });
    this.sens = Math.min(1.5, Math.max(0.6, this.sens * (clipped ? SENS_DOWN : SENS_UP)));
    for (let z = 0; z < out.length; z++) {
      for (let m = 0; m < out.length; m++) {
        out[m] = Math.max(out[m], out[z] / Math.pow(SPREAD, Math.abs(m - z)));
      }
    }
    return out;
  }
  _bands(n, t) {
    if (this.analyser) {
      this.analyser.getByteFrequencyData(this.freqData);
      const binHz = this.audioCtx.sampleRate / this.analyser.fftSize;
      const raw = [];
      for (let i = 0; i < BAND_COUNT; i++) {
        const lo = FREQ_MIN * Math.pow(FREQ_MAX / FREQ_MIN, i / BAND_COUNT);
        const hi = FREQ_MIN * Math.pow(FREQ_MAX / FREQ_MIN, (i + 1) / BAND_COUNT);
        const loBin = Math.floor(lo / binHz);
        const hiBin = Math.min(Math.max(Math.floor(hi / binHz), loBin + 1), this.freqData.length);
        let sum = 0;
        for (let j = loBin; j < hiBin; j++) sum += this.freqData[j];
        const center = Math.sqrt(lo * hi);
        // Back to dB, tilt, then the app's -68..-30 window, gain and curve.
        // Normalizing over the analyser's full -100..-30 window instead would
        // let the tilt fabricate a phantom staircase out of digital silence;
        // the -68 floor clamps tilted silence to zero like the app does.
        const db =
          this.analyser.minDecibels +
          (sum / (hiBin - loBin) / 255) * (this.analyser.maxDecibels - this.analyser.minDecibels) +
          TILT_DB_PER_OCT * Math.log2(center / FREQ_MIN);
        const norm = Math.min(1, Math.max(0, (db + 68) / 38));
        raw.push(Math.pow(Math.min(1, norm * 1.3), 0.7));
      }
      return COLUMN_GROUPS.map(([a, b]) => raw.slice(a, b).reduce((s, v) => s + v, 0) / (b - a));
    }
    const talking = Math.max(0, Math.sin(t * 0.9) + 0.75) / 1.75;
    const out = [];
    for (let i = 0; i < n; i++) {
      const f = 2.1 + i * 1.7;
      const v = Math.max(0, Math.sin(t * f + i * 2.6) * 0.5 + 0.5 - i * 0.07);
      out.push(Math.min(1, v * talking));
    }
    return out;
  }
  // Target lit level for a dot, before ballistics.
  _targetLit(c, r, levels, t) {
    if (this.beat === 'bloom') {
      // A white wave sweeping bottom-to-top, then holding bright.
      if (this.reduceMotion) return 0.8;
      const p = Math.min(1, (performance.now() - this.beatSince) / 380);
      const wave = p * (ROWS + 1.4) - (ROWS - 1 - r);
      return Math.max(0, Math.min(1, wave));
    }
    if (this.beat === 'cancel') return 0;
    if (this.beat === 'error') return 0.5;
    if (this.phase === 'recording') {
      return Math.min(1, Math.max(0, (levels[c] ?? 0) * 3.6 - (ROWS - 1 - r)));
    }
    if (this.phase === 'transcribing') {
      if (this.reduceMotion) return 0.35;
      const scan = ((t * 4) % (COLS + 2)) - 1;
      return Math.max(0, 1 - Math.abs(c - scan) / 1.5) * 0.9;
    }
    if (this.phase === 'polishing') {
      if (this.reduceMotion) return 0.35;
      const step = t * 2.5;
      const i0 = Math.floor(step);
      const f = step - i0;
      const rnd = (k) => {
        const x = Math.sin((c * 7.3 + r * 13.7) * 127.1 + k * 311.7) * 43758.5453;
        return x - Math.floor(x);
      };
      const ease = f < 0.5 ? f * 2 : (1 - f) * 2;
      const a0 = rnd(i0) > 0.7 ? 1 : 0;
      const a1 = rnd(i0 + 1) > 0.7 ? 1 : 0;
      return Math.max(a0 * (1 - f), a1 * f) * (0.55 + 0.45 * ease);
    }
    return 0;
  }
  _fitWidth(text) {
    if (text === this._lastText) return;
    this._lastText = text;
    this._measurer.textContent = text;
    // The pill is 172px for the whole normal flow; it grows only when a text
    // (a long error) genuinely needs more room.
    const needed = 28 + 34.5 + 11 + Math.ceil(this._measurer.getBoundingClientRect().width) + 2;
    this._pill.style.width = `${Math.max(172, needed)}px`;
  }
  _frame(now) {
    if (!this._pill) return;
    const t = (now - this._t0) / 1000;
    const levels = this._bands(COLS, t);
    const targetColor = COLORS[this.beat ?? this.phase] ?? COLORS.recording;
    // Color lerps toward the phase target; ~180ms to converge.
    for (let i = 0; i < 3; i++) this.liveColor[i] += (targetColor[i] - this.liveColor[i]) * 0.22;
    const col = this.liveColor.map(Math.round).join(',');
    this._dots.forEach((d, i) => {
      const c = i % COLS;
      const r = Math.floor(i / COLS);
      const target = this._targetLit(c, r, this._meterBands(levels), t);
      // Fast attack, slow decay: speech reads as motion, never strobing.
      const rate = target > this.smooth[i] ? 0.55 : 0.14;
      this.smooth[i] += (target - this.smooth[i]) * rate;
      const lit = this.smooth[i];
      const a = 0.24 + lit * 0.76;
      d.style.background = `rgba(${col},${a.toFixed(2)})`;
      d.style.boxShadow =
        lit > 0.3 ? `0 0 ${4 + lit * 6}px rgba(${col},${(lit * 0.95).toFixed(2)})` : 'none';
    });
    const on = this.phase === 'recording' && !this.beat && !this.reduceMotion;
    this._aurora.querySelectorAll('i').forEach((b, i) => {
      const level = on ? (levels[Math.min(i * 2, levels.length - 1)] ?? 0) : 0;
      // Below this the blob's shadow reads as a smudge, not a glow.
      if (!on || level < 0.08) {
        b.style.boxShadow = 'none';
        return;
      }
      const drift = Math.sin(t * 0.4 + +b.dataset.seed) * 14;
      b.style.transform = `translateX(${drift}px)`;
      const intensity = 0.1 + level * 0.3;
      b.style.boxShadow = `0 0 18px 8px color-mix(in srgb, ${b.dataset.color} ${Math.round(intensity * 60)}%, transparent)`;
    });
    this._pill.style.boxShadow = on
      ? `inset 0 0 10px 1px rgba(255,59,77,${0.05 + levels[0] * 0.12}), inset 0 0 10px 1px rgba(224,69,158,${0.04 + (levels[2] ?? 0) * 0.1})`
      : 'none';
    if (this.beat === 'error') {
      this._label.textContent = 'Transcription failed';
      this._label.classList.add('status');
      this._time.style.display = 'none';
      this._fitWidth(this._label.textContent);
      return;
    }
    if (this.phase === 'recording') {
      this._label.textContent = '';
      this._label.classList.remove('status');
      this._time.style.display = 'block';
      const secs = Math.floor((now - this.phaseSince) / 1000);
      this._time.textContent = `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
      this._fitWidth('');
    } else {
      this._label.textContent =
        this.phase === 'transcribing' ? 'Transcribing' : this.phase === 'polishing' ? 'Polishing' : '';
      this._label.classList.add('status');
      this._time.style.display = 'none';
      this._fitWidth(this._label.textContent);
    }
  }
}
customElements.define('dk-pill', DkPill);
