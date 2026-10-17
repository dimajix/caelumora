// What is new to the user in the channels they read (issue 404).
//
// The server counts it as a channel is listed or opened (`unread`), over what
// the user may read: the messages others said that one hears, after the last
// one the user read. The session list counts on from there with every such
// message the global stream announces (`said`), and the chat view marks a
// channel read as it shows its newest message of someone else. The mark only
// moves forward on the server; what another tab or device read is seen here
// on the next listing.

import {api} from './api';
import {actorKey} from './mention';
import {isOwn} from './messages';
import type {ActorId, Message, Unread} from './types';

/** More new messages than this are shown as such. */
const MOST_SHOWN = 99;

/** How `count` new messages are shown: up to 99, more as such. */
export function unreadLabel(count: number): string {
  return count > MOST_SHOWN ? `${MOST_SHOWN}+` : String(count);
}

/** Whether `message` was said by someone other than `me`. */
export function isOthers(message: Message, me: ActorId): boolean {
  return message.sender_id !== undefined && !isOwn(message, me);
}

/** Whether `message` mentions `me` - by name or as one of everyone. */
export function mentions(message: Message, me: ActorId): boolean {
  return (message.mentions ?? []).some(mentioned => actorKey(mentioned) === actorKey(me));
}

/** `unread` with `message` of someone else counted: one more, and mentioned if it mentions `me`. */
export function countedIn(unread: Unread | undefined, message: Message, me: ActorId): Unread {
  return {
    messages: Math.min((unread?.messages ?? 0) + 1, MOST_SHOWN + 1),
    mentioned: (unread?.mentioned ?? false) || mentions(message, me),
    read_up_to: unread?.read_up_to,
  };
}

type ReadListener = (channelId: string, messageId: string) => void;
const readListeners = new Set<ReadListener>();

/** Calls `listener` whenever a channel was marked read here, until the returned function is called. */
export function onChannelRead(listener: ReadListener): () => void {
  readListeners.add(listener);
  return () => {
    readListeners.delete(listener);
  };
}

/** Marks `channelId` read up to `messageId`, then tells whoever counts what is new in it. */
export async function markRead(sessionId: string, channelId: string, messageId: string): Promise<void> {
  await api.markRead(sessionId, channelId, messageId);
  readListeners.forEach(listener => listener(channelId, messageId));
}

/** Whether the browser can show notifications and the user has not decided yet whether it may. */
export function mayAskForNotifications(): boolean {
  return typeof Notification !== 'undefined' && Notification.permission === 'default';
}

/** Asks the user whether the browser may show notifications - only on something they did. */
export async function askForNotifications(): Promise<void> {
  if (mayAskForNotifications()) await Notification.requestPermission();
}

/** How much of a message a notification shows. */
const EXCERPT = 200;

/**
 * Shows `message`, which mentions the user, as a notification of the browser,
 * if they allowed it; `onOpen` when it is clicked.
 */
export function notifyMention(message: Message, channelName: string, onOpen: () => void): void {
  if (typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
  const content = message.content ?? '';
  const body = content.length > EXCERPT ? `${content.slice(0, EXCERPT)}…` : content;
  const notification = new Notification(`${message.sender_name || 'Someone'} in ${channelName}`, {body, tag: message.id});
  notification.onclick = () => {
    window.focus();
    onOpen();
    notification.close();
  };
}
