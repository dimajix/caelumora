// Server-sent event streams (issues 33, 34, 35).
//
// Both of the server's streams - a channel's and the global one - start with
// a named `snapshot` event when asked to, end shortly before the session
// token that authenticated them expires (announced by a named `expired`
// event), and are otherwise plain unnamed events. Every consumer handles
// them the same way, so it is done once, here.

import {api} from './api';

export interface StreamHandlers {
  /** The state the stream starts from; again after every reconnect. */
  onSnapshot: (data: string) => void;
  /** Every unnamed event. */
  onEvent: (data: string) => void;
}

/**
 * Opens the stream at `url` with a snapshot and keeps it open until the
 * returned function is called.
 *
 * On `expired` it reconnects at once: the new request carries the cookie the
 * keep-alive has renewed, and its snapshot restores what happened in between,
 * so the handover is invisible. A connection that broke unexpectedly is left
 * to the browser's own retry; one that is really closed checks the session,
 * whose `401` brings up the login page if that was the cause.
 */
export function openEventStream(url: string, handlers: StreamHandlers): () => void {
  let stopped = false;
  let current: EventSource | null = null;

  const connect = () => {
    const eventSource = new EventSource(`${url}${url.includes('?') ? '&' : '?'}snapshot=true`);
    current = eventSource;
    eventSource.addEventListener('snapshot', (event) => handlers.onSnapshot((event as MessageEvent).data));
    eventSource.onmessage = (event) => handlers.onEvent(event.data);

    eventSource.addEventListener('expired', () => {
      eventSource.close();
      if (!stopped) connect();
    });

    eventSource.onerror = (e) => {
      // The browser retries by itself; leave it alone as long as it does.
      if (eventSource.readyState === EventSource.CONNECTING) return;
      console.error('SSE Error', e);
      eventSource.close();
      // EventSource does not expose the status code; if the session has
      // expired, this request's 401 brings up the login page.
      void api.me();
    };
  };

  connect();
  return () => {
    stopped = true;
    current?.close();
  };
}
