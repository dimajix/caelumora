// What is new to the user in the channels they read (issue 404), and what waits for them there
// (issue 594), told beside the channels and as notifications of the browser.
//
// The server counts it as a channel is listed or opened (`unread`), over what
// the user may read: the messages others said that one hears, after the last
// one the user read. The session list counts on from there with every such
// message the global stream announces (`said`) that the view of the line shows
// the user (`isSaidInLine`, issue 557), and the chat view marks a
// channel read as it shows its newest message of someone else. The mark only
// moves forward on the server; what another tab or device read is seen here
// on the next listing.
//
// What waits for the user in a channel - approvals, questions put to them - is the server's to
// count (issue 594): the list fetches the session again as the global stream tells that it changed
// for them (`waiting_changed`), and notifies where more waits than before.

import {api} from './api';
import {isOwn, mentions} from './messages';
import type {ActorId, Message, Unread, Waiting} from './types';

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

/**
 * Calls `listener` whenever a channel was marked read here, until the returned function is called.
 */
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
  const content = message.content ?? '';
  const body = content.length > EXCERPT ? `${content.slice(0, EXCERPT)}…` : content;
  notify(`${message.sender_name || 'Someone'} in ${channelName}`, body, message.id, onOpen);
}

/**
 * How `waiting` is told: `1 approval, 2 sets of questions` - each set a call's, which may ask
 * several -; empty where nothing waits (issue 594).
 */
export function waitingLabel(waiting: Waiting | undefined): string {
  const counted = (count: number, one: string, many: string) => count > 0 ? [`${count} ${count === 1 ? one : many}`] : [];
  return [
    ...counted(waiting?.approvals ?? 0, 'approval', 'approvals'),
    ...counted(waiting?.questions ?? 0, 'set of questions', 'sets of questions'),
  ].join(', ');
}

/**
 * Whether more waits in `after` than in `before`: an approval or questions asked since (issue 594).
 */
export function waitsMore(before: Waiting | undefined, after: Waiting | undefined): boolean {
  return (after?.approvals ?? 0) > (before?.approvals ?? 0) || (after?.questions ?? 0) > (before?.questions ?? 0);
}

/**
 * Shows that `waiting` waits for the user in channel `channelId`, called `channelName` (issue 594),
 * as a notification of the browser, if they allowed it - in place of one shown of the channel
 * before; `onOpen` when it is clicked.
 */
export function notifyWaiting(waiting: Waiting, channelId: string, channelName: string, onOpen: () => void): void {
  notify(`Waiting for you in ${channelName}`, waitingLabel(waiting), `waiting:${channelId}`, onOpen);
}

/**
 * Shows a notification of the browser, if the user allowed it: `title` and `body`, replacing one
 * shown with `tag`; `onOpen` when it is clicked.
 */
function notify(title: string, body: string, tag: string | undefined, onOpen: () => void): void {
  if (typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
  const notification = new Notification(title, {body, tag});
  notification.onclick = () => {
    window.focus();
    onOpen();
    notification.close();
  };
}
