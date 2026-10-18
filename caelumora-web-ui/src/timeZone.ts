// The time zone a session is held in (issue 424): an IANA name such as
// `Europe/Berlin`, in which its agents are given the times of messages.
// Which names are valid is the server's to decide; this only offers the
// browser's.

/** The browser's own time zone - what a new session is preset to; `null` if it cannot say. */
export function browserTimeZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

let names: string[] | undefined;

/** The time zones the browser knows, offered for choosing; any other may be typed. */
export function knownTimeZones(): string[] {
  if (names === undefined) {
    try {
      names = Intl.supportedValuesOf('timeZone');
    } catch {
      names = [];
    }
  }
  return names;
}

/** How a time zone is shown: its name, `UTC` for none. */
export function timeZoneLabel(name: string | null | undefined): string {
  return name || 'UTC';
}
