// The attachments of a message being written (issue 282): each file uploaded as it is attached, so
// that it is sent with the message by its id - classified as the release picked as it was
// attached, since an upload takes its label with it.

import type React from 'react';
import {useCallback, useEffect, useRef, useState} from 'react';
import type {ChannelAttachments} from './api';
import type {Classification} from './confidentiality';
import type {ContentRef} from './types';

/** A file attached to a message not yet sent: uploading, uploaded, or failed with why. */
export interface DraftAttachment {
  /** Its own within the draft, as long as it is in it. */
  key: string;
  file: File;
  /** Where an image is previewed from - a local object URL -; `null` for any other file. */
  preview: string | null;
  /** How it was classified as it was attached; absent: as the channel is. */
  classification?: Classification;
  state: {uploading: true} | {uploaded: ContentRef} | {failed: string};
}

/** The attachments of a message being written, as `useAttachmentDraft` keeps them. */
export interface AttachmentDraft {
  items: DraftAttachment[];
  /** Adds `files`, each uploaded at once, classified `classification` - the channel's if absent. */
  add: (files: File[], classification?: Classification) => void;
  /** Adds the files pasted in `event` as `add` does, which then pastes nothing else; text is pasted as text. */
  paste: (event: React.ClipboardEvent, classification?: Classification) => void;
  /** Removes those of `keys`. */
  remove: (...keys: string[]) => void;
  /** The ids to send, once nothing is uploading any more; `null` while something still is, or failed and is not removed. */
  ids: () => string[] | null;
}

/**
 * The attachments of a message being written, uploaded through `attachments`: those of its
 * channel - another `attachments`, another channel, begins with none, and what was attached before
 * is let go of; its uploads expire on the server.
 */
export function useAttachmentDraft(attachments: ChannelAttachments): AttachmentDraft {
  const [held, setHeld] = useState<{of: ChannelAttachments; items: DraftAttachment[]}>({of: attachments, items: []});
  const items = held.of === attachments ? held.items : [];
  // The previews made for `attachments`, to be let go of with their attachments or the draft.
  const previews = useRef(new Map<string, string>());
  // What tells the attachments apart within the draft.
  const counter = useRef(0);
  // The channel the draft is of now, for what settles after the channel changed.
  const current = useRef(attachments);

  useEffect(() => {
    current.current = attachments;
    const made = previews.current;
    return () => {
      made.forEach(url => URL.revokeObjectURL(url));
      made.clear();
    };
  }, [attachments]);

  /** Changes the items held for `of`, begun anew where the draft held another channel's; nothing if `of` is no longer the draft's. */
  const change = useCallback((of: ChannelAttachments, changed: (items: DraftAttachment[]) => DraftAttachment[]) => {
    setHeld(prev => prev.of === of ? {of, items: changed(prev.items)} : of === current.current ? {of, items: changed([])} : prev);
  }, []);

  const add = useCallback((files: File[], classification?: Classification) => {
    for (const file of files) {
      counter.current += 1;
      const key = String(counter.current);
      const preview = file.type.startsWith('image/') ? URL.createObjectURL(file) : null;
      if (preview) previews.current.set(key, preview);
      const settle = (state: DraftAttachment['state']) =>
        change(attachments, prev => prev.map(item => item.key === key ? {...item, state} : item));
      change(attachments, prev => [...prev, {key, file, preview, classification, state: {uploading: true}}]);
      attachments.upload(file, classification)
        .then(uploaded => settle({uploaded}))
        .catch(error => settle({failed: error instanceof Error ? error.message : String(error)}));
    }
  }, [attachments, change]);

  const paste = useCallback((event: React.ClipboardEvent, classification?: Classification) => {
    const files = [...event.clipboardData.files];
    if (files.length === 0) return;
    event.preventDefault();
    add(files, classification);
  }, [add]);

  const remove = useCallback((...keys: string[]) => {
    for (const key of keys) {
      const url = previews.current.get(key);
      if (url) URL.revokeObjectURL(url);
      previews.current.delete(key);
    }
    change(attachments, prev => prev.filter(item => !keys.includes(item.key)));
  }, [attachments, change]);

  const ids = () => items.every(item => 'uploaded' in item.state)
    ? items.flatMap(item => 'uploaded' in item.state ? [item.state.uploaded.id] : [])
    : null;

  return {items, add, paste, remove, ids};
}
