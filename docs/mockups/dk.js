//! Shared mock components for the diktafon HTML mocks. Served over http from
//! the repo root; `lit` comes from esm.sh (pinned), everything else is local.
//! Components inherit the Signal tokens as CSS custom properties through the
//! shadow boundary, so each mock keeps its own `:root` and components stay
//! token-agnostic. Repeated or interactive structures live here; one-off
//! static cards stay light-DOM HTML in each mock file.

import { LitElement, html, css } from 'https://esm.sh/lit@3.3.3';

/// The page's `* { box-sizing: border-box }` reset stops at the shadow
/// boundary, so every component includes this: without it borders and padding
/// add to declared sizes (a 40px button renders 42px, the 172px pill 202px).
export const reset = css`
  *,
  *::before,
  *::after {
    box-sizing: border-box;
  }
`;

/// Solar Linear path data (the app bundles the same files under
/// `crates/diktafon/assets/icons/diktafon`), so an SVG is never copy-pasted.
export const ICONS = {
  copy: '<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5"><path d="M6 11C6 8.17157 6 6.75736 6.87868 5.87868C7.75736 5 9.17157 5 12 5H15C17.8284 5 19.2426 5 20.1213 5.87868C21 6.75736 21 8.17157 21 11V16C21 18.8284 21 20.2426 20.1213 21.1213C19.2426 22 17.8284 22 15 22H12C9.17157 22 7.75736 22 6.87868 21.1213C6 20.2426 6 18.8284 6 16V11Z"/><path d="M6 19C4.34315 19 3 17.6569 3 16V10C3 6.22876 3 4.34315 4.17157 3.17157C5.34315 2 7.22876 2 11 2H15C16.6569 2 18 3.34315 18 5"/></g>',
  play: '<path fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5" d="M20.4086 9.35258C22.5305 10.5065 22.5305 13.4935 20.4086 14.6474L7.59662 21.6145C5.53435 22.736 3 21.2763 3 18.9671L3 5.0329C3 2.72368 5.53435 1.26402 7.59661 2.38548L20.4086 9.35258Z"/>',
  pause:
    '<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5"><path d="M2 6C2 4.11438 2 3.17157 2.58579 2.58579C3.17157 2 4.11438 2 6 2C7.88562 2 8.82843 2 9.41421 2.58579C10 3.17157 10 4.11438 10 6V18C10 19.8856 10 20.8284 9.41421 21.4142C8.82843 22 7.88562 22 6 22C4.11438 22 3.17157 22 2.58579 21.4142C2 20.8284 2 19.8856 2 18V6Z"/><path d="M14 6C14 4.11438 14 3.17157 14.5858 2.58579C15.1716 2 16.1144 2 18 2C19.8856 2 20.8284 2 21.4142 2.58579C22 3.17157 22 4.11438 22 6V18C22 19.8856 22 20.8284 21.4142 21.4142C20.8284 22 19.8856 22 18 22C16.1144 22 15.1716 22 14.5858 21.4142C14 20.8284 14 19.8856 14 18V6Z"/></g>',
  trash:
    '<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5"><path d="M20.5001 6H3.5"/><path d="M18.8332 8.5L18.3732 15.3991C18.1962 18.054 18.1077 19.3815 17.2427 20.1907C16.3777 21 15.0473 21 12.3865 21H11.6132C8.95235 21 7.62195 21 6.75694 20.1907C5.89194 19.3815 5.80344 18.054 5.62644 15.3991L5.1665 8.5"/><path d="M9.5 11L10 16"/><path d="M14.5 11L14 16"/><path d="M6.5 6C6.55588 6 6.58382 6 6.60915 5.99936C7.43259 5.97849 8.15902 5.45491 8.43922 4.68032C8.44784 4.65649 8.45667 4.62999 8.47434 4.57697L8.57143 4.28571C8.65431 4.03708 8.69575 3.91276 8.75071 3.8072C8.97001 3.38607 9.37574 3.09364 9.84461 3.01877C9.96213 3 10.0932 3 10.3553 3H13.6447C13.9068 3 14.0379 3 14.1554 3.01877C14.6243 3.09364 15.03 3.38607 15.2493 3.8072C15.3043 3.91276 15.3457 4.03708 15.4286 4.28571L15.5257 4.57697C15.5433 4.62992 15.5522 4.65651 15.5608 4.68032C15.841 5.45491 16.5674 5.97849 17.3909 5.99936C17.4162 6 17.4441 6 17.5 6"/></g>',
  x: '<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5"><circle cx="12" cy="12" r="10"/><path d="M14.5 9.50002L9.5 14.5M9.49998 9.5L14.5 14.5"/></g>',
  accept:
    '<path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M5 13L9 17L19 7"/>',
  reject:
    '<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-width="1.5"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></g>',
  retranscribe:
    '<path fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M18.364 8.04928L17.6569 7.34217C14.5327 4.21798 9.46734 4.21798 6.34315 7.34217C3.21895 10.4664 3.21895 15.5317 6.34315 18.6559C9.46734 21.7801 14.5327 21.7801 17.6569 18.6559C19.4737 16.8391 20.234 14.3658 19.9377 11.9995M18.364 3.80664V8.04928H14.1213"/>',
};

export function iconSvg(name) {
  return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] ?? ''}</svg>`;
}

/// The macOS traffic lights, one element instead of three spans per window.
export class DkTraffic extends LitElement {
  static styles = [
    reset,
    css`
      :host { position: absolute; top: 16px; left: 16px; display: flex; gap: 8px; }
      span { width: 12px; height: 12px; border-radius: 50%; background: #43465e; }
      span:first-child { background: #ff5f57; }
    `,
  ];
  render() {
    return html`<span></span><span></span><span></span>`;
  }
}
customElements.define('dk-traffic', DkTraffic);

/// The kit's switch: a 36x20 track with a 16px thumb. Clicking toggles it, so
/// the mock demonstrates both states without hand-writing each.
export class DkSwitch extends LitElement {
  static properties = { on: { type: Boolean, reflect: true } };
  static styles = [
    reset,
    css`
      :host {
        width: 36px; height: 20px; border-radius: 10px; background: var(--switch-track);
        position: relative; flex-shrink: 0; cursor: pointer; display: block;
      }
      :host([on]) { background: var(--accent); }
      .thumb {
        position: absolute; top: 2px; left: 2px; width: 16px; height: 16px;
        border-radius: 50%; background: var(--on-surface); opacity: 0.9;
      }
      :host([on]) .thumb { left: auto; right: 2px; opacity: 1; }
    `,
  ];
  constructor() {
    super();
    this.on = false;
  }
  connectedCallback() {
    super.connectedCallback();
    this.addEventListener('click', () => (this.on = !this.on));
  }
  render() {
    return html`<div class="thumb"></div>`;
  }
}
customElements.define('dk-switch', DkSwitch);

/// A 28px row icon button: copy, play, stop, delete. `active` tints it the
/// polishing magenta, `danger` turns the hover red.
export class DkIconBtn extends LitElement {
  static properties = {
    icon: { type: String },
    label: { type: String },
    active: { type: Boolean, reflect: true },
    danger: { type: Boolean, reflect: true },
    accent: { type: Boolean, reflect: true },
  };
  static styles = [
    reset,
    css`
      button {
        width: 28px; height: 28px; border: none; border-radius: 6px; padding: 0;
        display: grid; place-items: center; color: var(--faint, #7f84a2);
        background: none; font: inherit; cursor: pointer;
      }
      button:hover {
        background: color-mix(in srgb, var(--outline) 60%, transparent);
        color: var(--on-surface);
      }
      :host([active]) button { color: var(--status-live); }
      :host([danger]) button:hover { color: var(--signal-red); }
      :host([accent]) button { color: var(--accent); }
      :host([accent]) button:hover {
        background: color-mix(in srgb, var(--accent) 18%, transparent);
        color: var(--accent-hover);
      }
      svg { width: 14px; height: 14px; display: block; }
    `,
  ];
  render() {
    return html`<button aria-label=${this.label ?? this.icon} .innerHTML=${iconSvg(this.icon)}></button>`;
  }
}
customElements.define('dk-icon-btn', DkIconBtn);

/// A labeled control row: label + help text on the left, the control slotted
/// on the right. The flex row lives on an inner wrapper for the same reason
/// as the badge: `:host([divided])` padding would lose to the page reset.
export class DkControlRow extends LitElement {
  static properties = {
    label: { type: String },
    help: { type: String },
    divided: { type: Boolean, reflect: true },
  };
  static styles = [
    reset,
    css`
      :host { display: block; }
      .wrap { display: flex; align-items: center; justify-content: space-between; gap: 24px; }
      :host([divided]) .wrap { padding-top: 16px; border-top: 1px solid var(--outline); }
      .text .label { display: block; font-size: 15px; font-weight: 500; }
      .text .help { font-size: 13px; color: var(--on-surface-muted); margin-top: 4px; }
      ::slotted(*) { flex-shrink: 0; }
    `,
  ];
  render() {
    return html`
      <div class="wrap">
        <div class="text"><span class="label">${this.label}</span><div class="help">${this.help}</div></div>
        <slot></slot>
      </div>
    `;
  }
}
customElements.define('dk-control-row', DkControlRow);

/// A labeled field: label + control + help stacked. `warning` tints the help
/// line in the warning color, for a notice rather than a description.
export class DkField extends LitElement {
  static properties = {
    label: { type: String },
    help: { type: String },
    warning: { type: Boolean, reflect: true },
  };
  static styles = [
    reset,
    css`
      :host { display: block; }
      .label { font-size: 15px; font-weight: 500; }
      .control { margin-top: 8px; }
      .help { font-size: 13px; color: var(--on-surface-muted); margin-top: 8px; }
      :host([warning]) .help { color: var(--warning); }
    `,
  ];
  render() {
    return html`
      <div class="label">${this.label}</div>
      <div class="control"><slot></slot></div>
      ${this.help ? html`<div class="help">${this.help}</div>` : ''}
    `;
  }
}
customElements.define('dk-field', DkField);

/// A static select look-alike: value text plus chevron. Same `:host` rule as
/// the badge: padding lives on the inner box, never the host.
export class DkSelect extends LitElement {
  static properties = { value: { type: String }, compact: { type: Boolean, reflect: true } };
  static styles = [
    reset,
    css`
      :host { display: block; font-size: 15px; }
      :host([compact]) { width: 220px; }
      .box {
        height: 40px; border: 1px solid var(--outline); border-radius: 6px;
        display: flex; align-items: center; justify-content: space-between;
        padding: 0 12px; background: var(--surface);
      }
      .chev { color: var(--on-surface-muted); }
    `,
  ];
  render() {
    return html`<div class="box"><span>${this.value}</span><span class="chev">⌄</span></div>`;
  }
}
customElements.define('dk-select', DkSelect);

/// A row of choices with the current one raised, the gpui `segmented()`.
/// `options` is a comma-separated list, `selected` the active index. The
/// track lives on an inner wrapper, never `:host`.
export class DkSegmented extends LitElement {
  static properties = { options: { type: String }, selected: { type: Number } };
  static styles = [
    reset,
    css`
      :host { display: block; }
      .track {
        display: flex; gap: 2px; padding: 3px; border-radius: 8px;
        background: var(--surface-sunken); border: 1px solid var(--outline);
      }
      button {
        flex: 1; height: 30px; border: none; border-radius: 5px; background: none;
        font: inherit; font-size: 13px; color: var(--on-surface-muted); cursor: pointer;
      }
      button:hover { background: color-mix(in srgb, var(--outline) 60%, transparent); }
      button[aria-pressed='true'] { background: var(--surface-raised); color: var(--on-surface); font-weight: 500; }
    `,
  ];
  constructor() {
    super();
    this.options = '';
    this.selected = 0;
  }
  render() {
    const labels = this.options.split(',').map((label) => label.trim());
    return html`
      <div class="track">
        ${labels.map(
          (label, index) => html`<button
            aria-pressed=${index === this.selected}
            @click=${() => (this.selected = index)}
          >${label}</button>`,
        )}
      </div>
    `;
  }
}
customElements.define('dk-segmented', DkSegmented);

/// A button. `primary` fills it with the accent.
export class DkButton extends LitElement {
  static properties = { primary: { type: Boolean, reflect: true } };
  static styles = [
    reset,
    css`
      button {
        height: 40px; padding: 0 16px; display: flex; align-items: center; flex-shrink: 0;
        border: 1px solid var(--outline); border-radius: 6px; background: var(--surface-raised);
        font-size: 15px; font-weight: 500; color: inherit; cursor: pointer; font-family: inherit;
        transition: background-color 120ms ease, border-color 120ms ease, transform 160ms cubic-bezier(0.23, 1, 0.32, 1);
      }
      button:hover { background: var(--raised-hover); border-color: var(--outline-hover); }
      button:active { transform: scale(0.97); }
      :host([primary]) button { background: var(--accent); border-color: transparent; color: var(--on-accent); }
      :host([primary]) button:hover { background: var(--accent-hover); border-color: transparent; }
    `,
  ];
  render() {
    return html`<button part="button"><slot></slot></button>`;
  }
}
customElements.define('dk-button', DkButton);

/// A permission state badge: dot in the state's semantic color plus a label.
/// The pill lives on an inner wrapper, never on `:host`: document styles beat
/// `:host` rules, so the page's `* { padding: 0 }` reset would zero any host
/// padding. Box styles stay inside the shadow root; `:host` keeps only display
/// and inherited type.
export class DkBadge extends LitElement {
  static properties = { state: { type: String } };
  static styles = [
    reset,
    css`
      :host { display: inline-block; font-size: 13px; font-weight: 500; color: var(--on-surface-muted); }
      .pill {
        display: inline-flex; align-items: center; gap: 8px; height: 24px; padding: 0 8px;
        border-radius: 12px; background: var(--surface-raised); border: 1px solid var(--outline);
      }
      .dot { width: 8px; height: 8px; border-radius: 50%; }
      :host([state='granted']) .dot { background: var(--status-ok); }
      :host([state='pending']) .dot { background: var(--ring-idle); }
      :host([state='missing']) .dot { background: var(--signal-red); }
    `,
  ];
  render() {
    return html`<span class="pill"><span class="dot"></span><slot></slot></span>`;
  }
}
customElements.define('dk-badge', DkBadge);

/// A sidebar navigation item: glyph, label, active highlight.
export class DkNavItem extends LitElement {
  static properties = { icon: { type: String }, active: { type: Boolean, reflect: true } };
  static styles = [
    reset,
    css`
      :host {
        height: 40px; display: flex; align-items: center; gap: 12px; padding: 0 12px;
        border-radius: 8px; color: var(--on-surface-muted); font-size: 15px;
      }
      :host([active]) { background: var(--surface-raised); color: var(--on-surface); }
      .icon { width: 16px; text-align: center; opacity: 0.8; }
    `,
  ];
  render() {
    return html`<span class="icon">${this.icon}</span><slot></slot>`;
  }
}
customElements.define('dk-nav-item', DkNavItem);

/// A specimen caption: bold number plus annotation.
export class DkCaption extends LitElement {
  static properties = {};
  static styles = [
    reset,
    css`
      :host {
        display: block; width: 520px; font-size: 13px; color: var(--faint);
        margin-bottom: -28px;
      }
      ::slotted(b) { color: var(--on-surface-muted); font-weight: 500; }
    `,
  ];
  render() {
    return html`<slot></slot>`;
  }
}
customElements.define('dk-caption', DkCaption);

/// Onboarding progress dots. `steps` total, `at` lit.
export class DkSteps extends LitElement {
  static properties = { steps: { type: Number }, at: { type: Number } };
  static styles = [
    reset,
    css`
      :host { display: flex; gap: 8px; align-items: center; justify-content: center; margin-top: 4px; }
      .step { width: 8px; height: 8px; border-radius: 50%; background: var(--ring-idle); opacity: 0.4; }
      .step.on { opacity: 1; background: var(--on-surface); }
    `,
  ];
  constructor() {
    super();
    this.steps = 3;
    this.at = 0;
  }
  render() {
    return html`${Array.from({ length: this.steps }, (_, i) => html`<span class="step ${i === this.at ? 'on' : ''}"></span>`)}`;
  }
}
customElements.define('dk-steps', DkSteps);
