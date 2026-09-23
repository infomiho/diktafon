//! Onboarding mock: the five panels rendered from data. Each panel is traffic,
// an optional mark, a title, prose, an optional permission row, one action,
// and the progress dots, so a copy change touches one object, not five
//! hand-written panels.

import { LitElement, html, css } from 'https://esm.sh/lit@3.3.3';
import './dk.js?v=2';

const PANELS = [
  {
    caption: ['1.', 'The only screen with the mark.'],
    mark: true,
    title: 'Welcome to diktafon',
    body: 'Diktafon needs two permissions from macOS. Dictation runs on this Mac and nothing is uploaded.',
    action: 'Get started',
    at: 0,
  },
  {
    caption: ['2.', 'No mark from here on; the title carries the screen.'],
    title: 'Allow the microphone',
    body: 'Diktafon uses the microphone to record your dictation.',
    perm: { name: 'Microphone', badge: 'pending', badgeText: 'Not asked' },
    action: 'Allow microphone',
    at: 1,
  },
  {
    caption: ['2, granted.', ''],
    title: 'Allow the microphone',
    body: 'Diktafon uses the microphone to record your dictation.',
    perm: { name: 'Microphone', badge: 'granted', badgeText: 'Granted' },
    action: 'Continue',
    at: 1,
  },
  {
    caption: ['3.', 'macOS keeps this one behind a switch.'],
    title: 'Allow Accessibility',
    body: 'Diktafon uses Accessibility to paste text into other apps.',
    perm: { name: 'Accessibility', badge: 'missing', badgeText: 'Not granted' },
    action: 'Open System Settings',
    at: 2,
  },
  {
    caption: ['3, granted.', 'The last screen starts the app.'],
    title: 'Allow Accessibility',
    body: 'Diktafon uses Accessibility to paste text into other apps.',
    perm: { name: 'Accessibility', badge: 'granted', badgeText: 'Granted' },
    action: 'Start dictating',
    at: 2,
  },
];

export class DkOnboarding extends LitElement {
  static styles = css`
    :host { display: flex; flex-direction: column; align-items: center; gap: 40px; }
    /* A window of its own: on first run there is nothing behind it, and it
       closes like any window. */
    .panel {
      position: relative; width: 520px; padding: 48px 32px 32px; border-radius: 8px;
      background: var(--background); border: 1px solid var(--outline);
      display: flex; flex-direction: column; gap: 16px;
    }
    .mark { width: 64px; height: 36px; margin-bottom: 8px; }
    .panel h1 {
      font-family: var(--font-display); font-size: 24px; font-weight: 600;
      line-height: 1.2; margin: 0;
    }
    .panel p { font-size: 15px; color: var(--on-surface-muted); max-width: 46ch; margin: 0; }
    /* The action owns the width: it is the only thing to press. */
    dk-button { display: block; margin-top: 8px; }
    dk-button::part(button) { width: 100%; justify-content: center; }
    /* Permissions: one grouped list. The dot leads the name. */
    .perm-list { border: 1px solid var(--outline); border-radius: 8px; overflow: hidden; }
    .perm { display: flex; align-items: center; justify-content: space-between; gap: 24px; padding: 12px 16px; }
    .perm-name { display: flex; align-items: center; gap: 10px; font-size: 15px; font-weight: 500; }
  `;
  render() {
    return html`${PANELS.map(
      (p) => html`
        <dk-caption><b>${p.caption[0]}</b> ${p.caption[1]}</dk-caption>
        <div class="panel">
          <dk-traffic></dk-traffic>
          ${p.mark
            ? html`<img class="mark" src="../../assets/diktafon-mark-flat.svg" width="64" height="36" alt="" />`
            : ''}
          <h1>${p.title}</h1>
          <p>${p.body}</p>
          ${p.perm
            ? html`<div class="perm-list">
                <div class="perm">
                  <div class="perm-name">
                    ${p.perm.name}<dk-badge state=${p.perm.badge}>${p.perm.badgeText}</dk-badge>
                  </div>
                </div>
              </div>`
            : ''}
          <dk-button primary>${p.action}</dk-button>
          <dk-steps steps="3" at=${p.at}></dk-steps>
        </div>
      `,
    )}`;
  }
}
customElements.define('dk-onboarding', DkOnboarding);
