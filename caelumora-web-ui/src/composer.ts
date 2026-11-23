// What a user is writing in a conversation (issue 595): the text, kept as a draft across a
// re-login, what is attached to it, and how it is classified - held by whatever shows the field to
// write in, which files dropped onto it reach as well.

import {useEffect, useState} from 'react';
import type {ChannelAttachments} from './api';
import {useAttachmentDraft, type AttachmentDraft} from './attachmentDraft';
import type {Classification} from './confidentiality';
import {loadDraft, saveDraft} from './session';

/** What a user is writing, as `useComposer` keeps it. */
export interface ComposerState {
  text: string;
  setText: (change: string | ((text: string) => string)) => void;
  /** What is attached to it (issue 282). */
  draft: AttachmentDraft;
  /** How it is classified; `null`: as the channel (issue 133). */
  release: Classification | null;
  setRelease: (classification: Classification | null) => void;
}

/**
 * What a user is writing under `draftKey`, its attachments uploaded to `attachments`. The text is
 * restored from session storage as the field is first shown, so a forced re-login does not lose
 * it; a release holds where it was chosen alone.
 */
export function useComposer(draftKey: string, attachments: ChannelAttachments): ComposerState {
  const [text, setText] = useState(() => loadDraft(draftKey));
  const draft = useAttachmentDraft(attachments);
  const [chosenRelease, setChosenRelease] = useState<{key: string; classification: Classification} | null>(null);
  const release = chosenRelease?.key === draftKey ? chosenRelease.classification : null;
  const setRelease = (classification: Classification | null) =>
    setChosenRelease(classification && {key: draftKey, classification});
  useEffect(() => {
    saveDraft(draftKey, text);
  }, [draftKey, text]);
  return {text, setText, draft, release, setRelease};
}
