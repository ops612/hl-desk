// Clipboard feature. Called once, the first time the Clipboard tab is shown.
export function mount(el: HTMLElement): void {
  el.innerHTML = '<p class="empty">Clipboard: coming soon.</p>';
}
