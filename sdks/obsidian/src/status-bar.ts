// Status-bar widget — visualizes the controller's state machine in the
// Obsidian UI. Pure DOM manipulation, no Obsidian-specific imports beyond
// the HTMLElement passed in by the host (`addStatusBarItem()`'s return).

export type SyncState = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'error';

const LABELS: Record<SyncState, string> = {
  idle: 'Agentsync: idle',
  connecting: 'Agentsync: connecting…',
  connected: 'Agentsync: connected',
  reconnecting: 'Agentsync: reconnecting…',
  error: 'Agentsync: error',
};

export class StatusBar {
  constructor(private readonly el: HTMLElement) {
    this.el.addClass('agentsync-status');
    this.set('idle');
  }

  set(state: SyncState, detail?: string): void {
    const text = detail ? `${LABELS[state]} (${detail})` : LABELS[state];
    this.el.setText(text);
    this.el.removeClass('agentsync-state-idle');
    this.el.removeClass('agentsync-state-connecting');
    this.el.removeClass('agentsync-state-connected');
    this.el.removeClass('agentsync-state-reconnecting');
    this.el.removeClass('agentsync-state-error');
    this.el.addClass(`agentsync-state-${state}`);
  }

  onClick(handler: () => void): void {
    this.el.addClass('mod-clickable');
    this.el.addEventListener('click', handler);
  }
}
