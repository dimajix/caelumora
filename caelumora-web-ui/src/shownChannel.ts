// What the messages shown of a channel need of it (issue 536): provided once by whatever shows
// them - the chat, the context and history panels -, and read where it is used - by a message, the
// card of a tool call, the handler of a presentation -, rather than handed down through each of
// them.

import {createContext, useContext} from 'react';
import type {ChannelArtifacts, ChannelAttachments, WorkspaceFiles} from './api';
import type {Page} from './page';

/**
 * What the messages shown of a channel need of it; what is absent, they do without. Outside the
 * chat, in a panel that is the record of what was seen, heard and said, only the attachments are.
 */
export interface ShownChannel {
  /**
   * The attachments of the channel's messages, where they are downloaded (issue 282); named only
   * without them.
   */
  attachments?: ChannelAttachments;
  /**
   * The files of the channel's workspace as the user may use them (issue 273); absent outside the
   * chat.
   */
  files?: WorkspaceFiles;
  /** Changes whenever those files may have changed. */
  filesVersion?: number;
  /** The artifacts of the channel as the user reaches them (issue 271); absent outside the chat. */
  artifacts?: ChannelArtifacts;
  /**
   * Opens a page in the place of the channel - a skill's (issue 281); absent outside the chat.
   */
  onSelectPage?: (page: Page) => void;
  /**
   * Ends a command a call handed back as running (issue 294); absent outside the chat, and where
   * the user may not change the workspace's files.
   */
  onKillCommand?: (commandId: string) => void;
  /**
   * The commands a result in the channel, or the user, ended: none is offered to be ended again.
   */
  endedCommands?: ReadonlySet<string>;
  /**
   * The digests of what the messages of each turn carry, by the turn's id: the cards of its calls
   * leave it to them (issue 535); absent outside the chat.
   */
  digestsByTurn?: ReadonlyMap<string, ReadonlySet<string>>;
}

/** The channel the messages below are shown of; without a provider, nothing of it. */
export const ShownChannelContext = createContext<ShownChannel>({});

/** The channel the messages being shown are of. */
export function useShownChannel(): ShownChannel {
  return useContext(ShownChannelContext);
}
