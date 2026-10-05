// The language a channel's conversation is held in and a user prefers
// (issue 56): a BCP 47 tag such as `de` or `en-GB`. Which tags are valid is
// the server's to decide; this only suggests common ones and names them.

/** Tags offered for choosing; any other may be typed. */
export const SUGGESTED_LANGUAGES = [
  'de', 'en', 'en-GB', 'en-US', 'fr', 'es', 'it', 'nl', 'pt', 'pt-BR', 'pl', 'cs',
  'sv', 'da', 'nb', 'fi', 'tr', 'uk', 'ru', 'ja', 'zh-Hans', 'zh-Hant', 'ko', 'ar',
];

let names: Intl.DisplayNames | null | undefined;

/** The browser's English names of languages; `null` where it has none. */
function displayNames(): Intl.DisplayNames | null {
  if (names === undefined) {
    try {
      names = new Intl.DisplayNames(['en'], {type: 'language'});
    } catch {
      names = null;
    }
  }
  return names;
}

/**
 * How a language is shown: its name and its tag, e.g. `German (de)`; the tag
 * alone if the browser has no name for it, `None` for no language.
 */
export function languageLabel(tag: string | null | undefined): string {
  if (!tag) return 'None';
  try {
    const name = displayNames()?.of(tag);
    return name && name !== tag ? `${name} (${tag})` : tag;
  } catch {
    // Not a tag the browser understands; the server will say whether it is one.
    return tag;
  }
}
