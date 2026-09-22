// Keeping a session alive while someone is watching (issue 31).
//
// The token is renewed by any request, but server-sent events are one long
// response and cannot renew anything: a user who only reads would be logged
// out mid-conversation. So the UI renews by itself while its tab is visible,
// and only then — a tab in the background is not someone watching.
//
// When to renew is read from the `caelumora_session_expires` cookie, which
// the server sets next to the (HttpOnly) token cookie. It carries no secret,
// only the expiry, and the browser updates it on every renewal — including
// one made by another tab, whose work this tab then sees.

import {useEffect} from 'react';
import {api} from './api';

export const SESSION_EXPIRES_COOKIE = 'caelumora_session_expires';

/** When the current session expires, in seconds since the epoch. */
export function sessionExpiresAt(): number | null {
  try {
    const entry = document.cookie.split(';')
      .map(part => part.trim())
      .find(part => part.startsWith(`${SESSION_EXPIRES_COOKIE}=`));
    const value = entry && Number(entry.slice(SESSION_EXPIRES_COOKIE.length + 1));
    return value && Number.isFinite(value) ? value : null;
  } catch {
    return null;
  }
}

/**
 * Seconds to wait before renewing again. The server renews a token once it is
 * past half its lifetime, so that is when the next attempt is due; the bounds
 * keep a missing or stale expiry cookie from turning this into a busy loop.
 */
export function nextRefreshDelay(lifetimeSecs: number, expiresAt: number | null, nowSecs: number): number {
  const minimum = Math.max(30, lifetimeSecs / 6);
  if (expiresAt === null) return Math.max(minimum, lifetimeSecs / 3);
  const due = expiresAt - lifetimeSecs / 2;
  return Math.min(Math.max(due - nowSecs, minimum), lifetimeSecs);
}

/**
 * Renews the session while the tab is visible. Does nothing without a
 * lifetime, i.e. while logged out.
 */
export function useSessionKeepAlive(lifetimeSecs: number | undefined) {
  useEffect(() => {
    if (!lifetimeSecs || lifetimeSecs <= 0) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let stopped = false;

    const schedule = () => {
      clearTimeout(timer);
      if (stopped || document.hidden) return;
      const delay = nextRefreshDelay(lifetimeSecs, sessionExpiresAt(), Date.now() / 1000);
      timer = setTimeout(renew, delay * 1000);
    };

    const renew = async () => {
      try {
        await api.refresh();
      } catch {
        // A 401 has already sent the app back to the login page.
      }
      schedule();
    };

    const onVisibilityChange = () => {
      if (document.hidden) {
        clearTimeout(timer);
      } else {
        // Catch up on what was missed while the tab was in the background.
        void renew();
      }
    };

    document.addEventListener('visibilitychange', onVisibilityChange);
    schedule();
    return () => {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [lifetimeSecs]);
}

/** The key under which a channel's unsent message is kept. */
function draftKey(channelId: string): string {
  return `caelumora.draft.${channelId}`;
}

/** The message typed but not sent in this channel, kept across a re-login. */
export function loadDraft(channelId: string): string {
  try {
    return sessionStorage.getItem(draftKey(channelId)) ?? '';
  } catch {
    return '';
  }
}

export function saveDraft(channelId: string, text: string) {
  try {
    if (text) {
      sessionStorage.setItem(draftKey(channelId), text);
    } else {
      sessionStorage.removeItem(draftKey(channelId));
    }
  } catch {
    // No session storage (private mode, blocked): drafts are then not kept.
  }
}
