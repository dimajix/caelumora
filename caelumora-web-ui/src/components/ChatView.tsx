// One channel as the chat shows it: what is the channel's - its participants and what they used,
// its approvals and questions, its notes, workspace and attachments, the panels and dialogs about
// it - and its line, to which it hands on what the channel's stream says of it (issue 595).

import React, {useEffect, useMemo, useRef, useState} from 'react';
import {
  ChevronDown,
  ChevronRight,
  Gauge,
  HardDrive,
  History,
  IdCard,
  Languages,
  Lock,
  LogOut,
  MessageCircleQuestion,
  MessageSquare,
  NotebookPen,
  Paperclip,
  Pencil,
  Scale,
  Shield,
  ShieldAlert,
  Square,
  Trash2,
  UserMinus,
  UserPlus,
  Wrench,
} from 'lucide-react';
import type {
  ActorId,
  ActorState,
  ApprovalRequest,
  ApprovalRule,
  ChannelParticipant,
  ChannelResponse,
  ChannelSnapshot,
  CommandDescription,
  Compartment,
  Envelope,
  HistoryPage,
  Message,
  PublishedNotes,
  QuestionRequest,
  QuestionResponse,
  TokenUsage,
  ToolCall,
  UserResponse,
  WorkspaceResponse,
} from '../types';
import {NotesList} from './NotesList';
import {ChannelAttachmentsPanel} from './ChannelAttachmentsPanel';
import {responderPolicyInfo} from '../moderator';
import {
  CONFIDENTIALITY_LEVELS,
  type Classification,
  classificationDescription,
  classificationLabel,
  levelsUpTo,
} from '../confidentiality';
import {ClassificationDialog} from './ClassificationDialog';
import {LanguageDialog} from './LanguageDialog';
import {ApprovalRulesDialog} from './ApprovalRulesDialog';
import {languageLabel} from '../language';
import {approvalRulesChangedChannel, changedChannel, descriptionChangedChannel, languageChangedChannel, languageChangedSession, joinedOrLeftChannel, onGlobalEvent, profileApprovalRules, reclassifiedChannel, saidInChannel, startedChannel} from '../globalEvents';
import {isOthers, markRead} from '../unread';
import {api} from '../api';
import type {Page} from '../page';
import {ApiRequestError, reportError} from '../errors';
import {cn} from '../classNames';
import {ShownChannelContext, type ShownChannel} from '../shownChannel';
import {attachedByTurn, endedCommands, formatArguments, formatTimestamp, formatTokens, formatTokensShort, isSaidInLine, isShownInLine, mergeToolResults} from '../messages';
import {TokenGauge} from './TokenGauge';
import {HoverCard} from './HoverCard';
import {ActionMenu, type MenuAction} from './ActionMenu';
import {ResizeHandle} from './ResizeHandle';
import {usePanelWidth} from '../panelWidth';
import {UsageStats} from './UsageStats';
import {ContextPanel} from './ContextPanel';
import {HistoryPanel} from './HistoryPanel';
import {ToolsPanel} from './ToolsPanel';
import {PrivateNotesPanel} from './PrivateNotesPanel';
import {ProfilePanel} from './ProfilePanel';
import {ChannelDescription} from './ChannelDescription';
import {ChannelWorkspacePanel} from './ChannelWorkspacePanel';
import {DescriptionDialog} from './DescriptionDialog';
import {AddParticipantDialog} from './AddParticipantDialog';
import {ApprovalRuleOffer} from './ApprovalRuleOffer';
import {QuestionCard} from './QuestionCard';
import {approveEverything, approvingEverything, initialChoice, ruleOf, type RuleChoice} from '../approvalRules';
import {APPROVER_NAME, actorKey, actorPath, agentIdOf, isApprover, sameActor, userActor, userIdOf} from '../actor';
import {openEventStream} from '../stream';
import {ConversationView} from './ConversationView';
import {ChatChannelContext, type ChatChannel} from '../chatChannel';
import {NO_CONVERSATION, answersUnderWay, conversationOf, shownOf, withEarlierPage, withEvent, withLastPage, withAdded, withReplaced, type Answer, type Conversation} from '../conversation';
import type {UserInput} from '../command';


/** The menu button of a participant's row: shown while the row is hovered or the button focused. */
const ROW_MENU = 'text-gray-400 opacity-0 group-hover:opacity-100 focus:opacity-100 hover:text-gray-700 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-700';

/** What an agent works at, in the states that count as working (issue 243). */
const WORKING: Partial<Record<ActorState, string>> = {
  Planning: 'Working: thinking',
  ExecutingTools: 'Working: running tools',
  Reflecting: 'Working: finishing its answer',
  Cancelling: 'Working: stopping',
  CarryingOutCommand: 'Working: carrying out a command',
  Busy: 'Working: on something of its own',
};

/** The states in which a cancel still ends an agent's answer (`ActorState::interruptible`). */
const INTERRUPTIBLE: ActorState[] = ['Planning', 'ExecutingTools', 'WaitingForUser', 'Busy'];

/**
 * The dot that shows where a participant stands (issue 243): an agent's state as
 * a colour, the state itself as its title - `Busy` with what it does of its
 * own accord, `activity`, as it said it last (issue 284). A user has none.
 */
function stateDot(participant: ChannelParticipant, activity?: string): {className: string; title: string} | null {
  if (participant.kind !== 'agent') return null;
  const working = participant.state === 'Busy' && activity ? `Working: ${activity}` : participant.state && WORKING[participant.state];
  // A command pulses in orange (issue 299), an answer in green.
  if (working) return {className: participant.state === 'CarryingOutCommand' ? 'bg-orange-500 animate-command' : 'bg-green-500 animate-working', title: working};
  switch (participant.state) {
    case 'Idle': return {className: 'bg-green-500', title: 'Idle'};
    case 'WaitingForUser': return {className: 'bg-amber-500', title: 'Waiting for a person'};
    case 'Sleeping': return {className: 'bg-indigo-500', title: 'Asleep'};
    default: return {className: 'bg-gray-400 dark:bg-gray-500', title: 'Not running'};
  }
}

/**
 * `usage` with the call that produced `message` added, as the server sums
 * them (`TokenUsage::of`, issue 209) - or `null` if the message reports no
 * tokens: a tool's result, an external agent's answer. Shown while a turn
 * runs - the steps of a plan are many calls - and replaced by what the
 * server read once it ended: a call whose message was not published at all
 * is missing until then.
 */
function withCall(usage: TokenUsage | undefined, message: Message): TokenUsage | null {
  if (message.prompt_tokens == null && message.completion_tokens == null) return null;
  const prompt = message.prompt_tokens ?? 0;
  const completion = message.completion_tokens ?? 0;
  return {
    calls: (usage?.calls ?? 0) + 1,
    prompt_tokens: (usage?.prompt_tokens ?? 0) + prompt,
    completion_tokens: (usage?.completion_tokens ?? 0) + completion,
    context_size: message.context_size,
    context_tokens: prompt + completion,
  };
}

/**
 * Whose name an actor has in `channel` - a participant's, or one's that left
 * (issue 164) - to name whose turn was given up (issue 258).
 */
function nameIn(channel: ChannelResponse | null): (actor: ActorId) => string | undefined {
  return actor => isApprover(actor)
    ? APPROVER_NAME
    : [...(channel?.participants ?? []), ...(channel?.former_participants ?? [])]
      .find(m => sameActor(m.id, actor))?.name;
}

interface ChatViewProps {
  sessionId: string;
  channelId: string;
  /** Who is logged in: only a participant follows the channel live. */
  user: UserResponse;
  /**
   * Opens a page in the place of the channel - a skill's, for a call that names one (issue 281).
   */
  onSelectPage: (page: Page) => void;
}

/** The tabs of the channel panel, in their order, each with its icon and label. */
const SIDE_TABS = [
  {id: 'channel', icon: MessageSquare, label: 'Channel'},
  {id: 'notes', icon: NotebookPen, label: 'Notes'},
  {id: 'workspace', icon: HardDrive, label: 'Files'},
  {id: 'attachments', icon: Paperclip, label: 'Attachments'},
] as const;

/** What the channel panel shows. */
type SideTab = typeof SIDE_TABS[number]['id'];

export const ChatView: React.FC<ChatViewProps> = ({ sessionId, channelId, user, onSelectPage }) => {
  // The user's actor, whose messages are their own (issue 407).
  const me = useMemo(() => userActor(user.id), [user.id]);
  // The channel's line as the chat shows it (issues 557, 595): what is said in it, what of another
  // thread the line's view shows, the answers under way in it, and its pages.
  const [line, setLine] = useState<Conversation>(NO_CONVERSATION);
  // Its messages, each tool result with the call it answers; and as the chat shows them, without
  // the calls presented `quiet` that need not be shown.
  const merged = useMemo(() => mergeToolResults(line.messages), [line.messages]);
  const shown = useMemo(() => shownOf(line.messages), [line.messages]);
  const [channel, setChannel] = useState<ChannelResponse | null>(null);
  const [notes, setNotes] = useState<PublishedNotes[]>([]);
  const [sideTab, setSideTab] = useState<SideTab>('channel');
  // Changes whenever what the channel's messages carry may have changed (issue 509): a message
  // with attachments said, a message changed, taken back or reclassified, the channel reclassified,
  // the stream started from a snapshot.
  const [attachmentsVersion, setAttachmentsVersion] = useState(0);
  // Changes whenever the session's workspace or its files may have changed
  // here: a tool call ended, the channel's switch was set (issue 253).
  const [workspaceVersion, setWorkspaceVersion] = useState(0);
  // The workspace's files as this user may read them (issue 23): what a call offers is downloaded
  // from them (issue 273).
  const workspaceId = channel?.workspace_id;
  const workspaceFiles = useMemo(() => workspaceId ? api.workspaceFiles(workspaceId) : null, [workspaceId]);
  // The workspace itself, and for which: `null` where it could not be fetched.
  const [fetchedWorkspace, setFetchedWorkspace] = useState<{id: string; workspace: WorkspaceResponse | null} | null>(null);
  // `undefined` while it is fetched, `null` where there is none to use.
  const workspace = !workspaceId ? null : fetchedWorkspace?.id === workspaceId ? fetchedWorkspace.workspace : undefined;
  // The commands the user ended here (issue 294), until a result shows them ended.
  const [killedCommands, setKilledCommands] = useState<{channelId: string; ids: ReadonlySet<string>} | null>(null);
  // The channel's artifacts as this user reaches them: what a call published is downloaded from
  // them (issue 271).
  const channelArtifacts = useMemo(() => api.channelArtifacts(sessionId, channelId), [sessionId, channelId]);
  // The attachments of the channel's messages (issue 282).
  const channelAttachments = useMemo(() => api.channelAttachments(sessionId, channelId), [sessionId, channelId]);
  // The widths of the channel panel and of the panel showing an agent's
  // context, history or tools (issue 231). The channel panel keeps the key
  // it had as the participants panel (issue 245), so that its width stays.
  const channelPanel = usePanelWidth('participants');
  const detailPanel = usePanelWidth('detail');
  const [compartments, setCompartments] = useState<Compartment[]>([]);
  // What each agent's model calls used (issue 209), by `actorKey`: from the
  // snapshot, added to by each of its calls as their messages arrive, and
  // fetched again for an agent whose turn ended or was given up. Kept apart
  // from the channel, which is fetched again without it.
  const [usage, setUsage] = useState<Record<string, TokenUsage>>({});
  // What each agent does of its own accord (issue 284), by `actorKey`, as
  // it said it last: the title of its dot while it is `Busy`.
  const [activities, setActivities] = useState<Record<string, string>>({});
  // The messages whose calls `usage` holds already: those of the snapshot,
  // and those added as they arrived - a message can arrive twice.
  const countedCalls = useRef<Set<string>>(new Set());
  const [pendingApprovals, setPendingApprovals] = useState<ApprovalRequest[]>([]);
  // The pending approvals this user answered, still waiting for the decision.
  const [answeredApprovals, setAnsweredApprovals] = useState<string[]>([]);
  // Of those, the ones a rule of this user's answered (issue 13), by id.
  const [answeredByRule, setAnsweredByRule] = useState<Record<string, string>>({});
  // The approval each answer of this user's was to, by the answer's event id (issue 567): what a
  // refusal of it names as its correlation, so that the request it was to is answered no longer.
  const answersByEvent = useRef<Record<string, string>>({});
  // The rule each card is to add with its answer, by request id.
  const [ruleChoices, setRuleChoices] = useState<Record<string, RuleChoice | null>>({});
  // The questions the agents' calls wait for the answers to (issue 165),
  // and those this user answered, until the agent closes them.
  const [pendingQuestions, setPendingQuestions] = useState<QuestionRequest[]>([]);
  const [answeredQuestions, setAnsweredQuestions] = useState<string[]>([]);
  // This user's own rules (issue 13) - for the brave mode, shown while it
  // lasts; they follow the global stream.
  const [myRules, setMyRules] = useState<ApprovalRule[]>([]);
  // Where the new messages began as the channel was opened (issue 404):
  // after the message last read, as the server said then, so that the line
  // stays where it was while the mark moves on as the user reads.
  const [newSince, setNewSince] = useState<{channelId: string; after: string | null; count: number} | null>(null);
  if (channel?.id === channelId && newSince?.channelId !== channelId) {
    setNewSince({channelId, after: channel.unread?.read_up_to ?? null, count: channel.unread?.messages ?? 0});
  }

  /**
   * Shows `page`, the last page of the history, in place of what is shown, as a snapshot would
   * (issue 218).
   */
  const showLastPage = (page: HistoryPage<Message>, nameOf: (actor: ActorId) => string | undefined) =>
    setLine(prev => withLastPage(prev, page, nameOf));

  // The agent message whose context is shown, with the channel it belongs
  // to - another channel shows none (issue 41).
  const [contextOf, setContextOf] = useState<{channelId: string; message: Message} | null>(null);
  // The agent whose history is shown, with the channel it belongs to (issue
  // 207); a context opened from it is shown on top and returns to it.
  const [historyOf, setHistoryOf] = useState<{channelId: string; participant: ChannelParticipant} | null>(null);
  // What is shown of one participant: its tools (issue 194), its private notes
  // (issue 233) or who it is (issue 244) - one at a time.
  const [participantPanel, setParticipantPanel] = useState<{channelId: string; participant: ChannelParticipant; shows: 'tools' | 'privateNotes' | 'profile'} | null>(null);
  // Whether one of them takes the place of the participants panel.
  const detailShown = contextOf?.channelId === channelId || historyOf?.channelId === channelId
    || participantPanel?.channelId === channelId;
  // Whether the former participants of the channel are unfolded (issue 164).
  const [showFormer, setShowFormer] = useState(false);
  // The commands this user may run here, for `/` completion (issue 28).
  const [commands, setCommands] = useState<CommandDescription[]>([]);
  // Whether the dialog to add a participant is open (issue 46).
  const [isAddingParticipant, setIsAddingParticipant] = useState(false);
  // What is being reclassified (issue 133): the channel, or one message.
  const [isChangingLanguage, setIsChangingLanguage] = useState(false);
  // Whether what the channel is for is being changed (issue 245).
  const [isDescribing, setIsDescribing] = useState(false);
  // The channel's approval rules (issue 13), shown - and changed by who may.
  const [isShowingRules, setIsShowingRules] = useState(false);
  const [reclassifying, setReclassifying] = useState<{kind: 'channel'} | {kind: 'message'; message: Message} | null>(null);

  // The server returns participants in no particular order.
  const participants: ChannelParticipant[] = [...(channel?.participants ?? [])].sort((l, r) => l.name.localeCompare(r.name));
  // What the channel's agents used, summed over those it shows (issue 209);
  // null while none reported anything.
  const channelUsage = participants.reduce<{calls: number; prompt: number; completion: number} | null>((sum, participant) => {
    const used = usage[actorKey(participant.id)];
    if (!used) return sum;
    const base = sum ?? {calls: 0, prompt: 0, completion: 0};
    return {calls: base.calls + used.calls, prompt: base.prompt + used.prompt_tokens, completion: base.completion + used.completion_tokens};
  }, null);

  // Whether the logged-in user takes part in this channel; `undefined` until
  // the channel is known. Joining or leaving changes it (the participants follow
  // the global stream), and the view follows.
  const takesPart = channel?.id === channelId
    ? channel.participants.some(m => userIdOf(m.id) === user.id)
    : undefined;

  // The compartments a message may be released to; none if they cannot be
  // fetched - the message is then the channel's.
  useEffect(() => {
    api.getCompartments()
      .then(setCompartments)
      .catch(error => reportError('Failed to load the compartments', error));
  }, []);

  useEffect(() => {
    if (!workspaceId) return;
    let current = true;
    api.getWorkspace(workspaceId)
      .then(found => { if (current) setFetchedWorkspace({id: workspaceId, workspace: found}); })
      .catch(error => {
        if (!current) return;
        reportError('Failed to fetch the workspace of the channel', error);
        setFetchedWorkspace({id: workspaceId, workspace: null});
      });
    return () => { current = false; };
  }, [workspaceId, workspaceVersion]);

  useEffect(() => {
    api.getCommands(sessionId, channelId)
      .then(setCommands)
      .catch(error => {
        // Someone who is not a participant has no commands here - and learns
        // that when writing; completion is only a help and stays silent.
        if (error instanceof ApiRequestError && error.status === 403) return;
        reportError('Failed to load the commands', error);
      });
  }, [sessionId, channelId]);

  // The user's rules, and the channel's, follow the global stream (issue 13).
  useEffect(() => {
    let cancelled = false;
    api.getProfile()
      .then(profile => { if (!cancelled) setMyRules(profile.approval_rules ?? []); })
      .catch(error => reportError('Failed to fetch the profile', error));
    const stop = onGlobalEvent(event => {
      const mine = profileApprovalRules(event);
      if (mine) { setMyRules(mine); return; }
      const changed = approvalRulesChangedChannel(event);
      if (changed && changed.channelId === channelId) setChannel(prev => prev && {...prev, approval_rules: changed.rules});
    });
    return () => { cancelled = true; stop(); };
  }, [channelId]);

  // The notes follow the global stream: a changed state of an agent in this
  // channel is announced without content, and the notes are fetched again
  // (issue 65).
  useEffect(() => {
    return onGlobalEvent(event => {
      if (changedChannel(event) !== channelId) return;
      api.getNotes(sessionId, channelId)
        .then(setNotes)
        .catch(error => reportError('Failed to load the notes', error));
    });
  }, [sessionId, channelId]);

  // The participants follow the global stream as well: someone joined or left, and
  // the channel is fetched again (issue 46). Whoever was removed can no
  // longer fetch it and keeps what they saw until they leave the channel. So
  // is a channel that starts - one that was unavailable runs again (issue 463).
  useEffect(() => {
    return onGlobalEvent(event => {
      if (joinedOrLeftChannel(event) !== channelId && startedChannel(event) !== channelId) return;
      api.getChannel(sessionId, channelId)
        .then(setChannel)
        .catch(error => {
          if (error instanceof ApiRequestError && error.status === 404) return;
          reportError('Failed to load the channel', error);
        });
    });
  }, [sessionId, channelId]);

  // Who reads the channel without taking part follows what others hear in it
  // through the global stream (issue 404), as it is stored; a participant
  // follows it through their own actor.
  useEffect(() => {
    if (takesPart !== false) return;
    return onGlobalEvent(event => {
      const said = saidInChannel(event);
      if (!said || said.channelId !== channelId || !isSaidInLine(said, me)) return;
      setLine(prev => withAdded(prev, said.message));
      if (said.message.attachments?.length) setAttachmentsVersion(v => v + 1);
    });
  }, [channelId, takesPart, me]);

  // A reclassified channel follows the global stream (issue 133): it may
  // have changed its messages too, which are fetched again. A reclassified
  // message comes with the channel's stream, as every change to one (issue
  // 239).
  useEffect(() => {
    return onGlobalEvent(event => {
      const description = descriptionChangedChannel(event);
      if (description) {
        if (description.channelId === channelId) setChannel(prev => prev && {...prev, summary: description.summary, briefing: description.briefing});
        return;
      }
      // Which language holds is the server's to say (issue 425): the channel
      // is fetched again when its own language or its session's changed.
      const language = languageChangedChannel(event);
      const sessionLanguage = languageChangedSession(event);
      if (language || sessionLanguage) {
        if (language?.channelId === channelId || sessionLanguage === sessionId) {
          api.getChannel(sessionId, channelId)
            .then(setChannel)
            .catch(error => reportError('Failed to load the channel', error));
        }
        return;
      }
      const reclassified = reclassifiedChannel(event);
      if (!reclassified || reclassified.channelId !== channelId) return;
      setChannel(prev => prev && {...prev, classification: reclassified.classification});
      setAttachmentsVersion(v => v + 1);
      // Its last page anew, as a snapshot would show it (issue 218).
      Promise.all([api.getChannel(sessionId, channelId), api.getHistory(sessionId, channelId)])
        .then(([found, page]) => showLastPage(page, nameIn(found)))
        .catch(error => reportError('Failed to load the messages', error));
    });
  }, [sessionId, channelId]);

  /** Asks the agent that said `msg` to answer again instead (issue 234). */
  const retry = async (msg: Message) => {
    try {
      await api.retryMessage(sessionId, channelId, msg.id!);
    } catch (error) {
      reportError('Failed to ask for another answer', error);
    }
  };

  /** Changes the user's own `msg` (issue 234); whether it was changed. */
  const modify = async (msg: Message, content: string, attachments: string[]) => {
    try {
      await api.modifyMessage(sessionId, channelId, msg.id!, content, attachments);
      return true;
    } catch (error) {
      reportError('Failed to edit the message', error);
      return false;
    }
  };

  /** Asks for an answer anew after the turn given up that the notice `msg` tells of (issue 259). */
  const retryGivenUp = async (msg: Message) => {
    const path = msg.sender_id ? participantPath(actorKey(msg.sender_id)) : undefined;
    if (!path || !msg.turn_id) return;
    try {
      await api.retryAnswer(sessionId, channelId, path, msg.turn_id);
    } catch (error) {
      reportError('Failed to ask for another answer', error);
    }
  };

  /** Takes the user's own `msg` back (issue 234). */
  const revoke = async (msg: Message) => {
    try {
      await api.revokeMessage(sessionId, channelId, msg.id!);
    } catch (error) {
      reportError('Failed to take the message back', error);
    }
  };


  /**
   * Deletes the agent `participant` (issues 164, 314): it leaves every channel, this one too, until
   * it is restored.
   */
  const deleteAgent = async (participant: ChannelParticipant) => {
    const agent = agentIdOf(participant.id);
    if (!agent) return;
    try {
      await api.deleteAgent(agent);
    } catch (error) {
      reportError(`Failed to delete ${participant.name}`, error);
    }
  };

  /** Shows `shows` of `participant` in place of the participants panel, closing its history. */
  const showOf = (participant: ChannelParticipant, shows: 'tools' | 'privateNotes' | 'profile') => {
    setHistoryOf(null);
    setParticipantPanel({channelId, participant, shows});
  };

  /** What can be done with `participant` of the channel: what it offers (issue 230). */
  const participantActions = (participant: ChannelParticipant): MenuAction[] => [
    // Who it is is public (issue 244).
    {
      label: 'Show profile', icon: IdCard, title: `Show who ${participant.name} is`,
      onSelect: () => showOf(participant, 'profile'),
    },
    ...contextActions(participant),
    ...(participant.can_read_tools ? [{
      label: 'Show tools', icon: Wrench, title: `Show the tools of ${participant.name}`,
      onSelect: () => showOf(participant, 'tools'),
    }] : []),
    // One's own goes by one's own act (issue 562); whoever may change the channel removes anyone.
    ...(participant.can_leave ? [{
      label: participant.kind === 'user' ? 'Leave channel' : 'Take out of channel',
      icon: participant.kind === 'user' ? LogOut : UserMinus, danger: true,
      title: participant.kind === 'user' ? 'Leave the channel; you stay in the session' : `Take ${participant.name} out of the channel`,
      onSelect: () => void takeOut(participant, 'leave'),
    }] : []),
    ...(channel?.can_change ? [{
      label: 'Remove from channel', icon: UserMinus, danger: true, title: `Remove ${participant.name} from the channel`,
      onSelect: () => void takeOut(participant, 'remove'),
    }] : []),
    ...(participant.can_delete ? [{
      label: 'Delete agent', icon: Trash2, danger: true, confirm: 'Delete agent? It leaves every channel until it is restored',
      title: `${participant.name} leaves every channel it takes part in, not only this one`,
      onSelect: () => void deleteAgent(participant),
    }] : []),
  ];

  /**
   * Reading what `participant` heard, said and kept to itself - for a former participant too -
   * where it may be read.
   */
  const contextActions = (participant: ChannelParticipant): MenuAction[] => participant.can_read_context ? [{
    label: 'Show history', icon: History, title: `Show what ${participant.name} heard and said`,
    onSelect: () => setHistoryOf({channelId, participant}),
  }, {
    label: 'Show private notes', icon: Lock, title: `Show what ${participant.name} noted for itself in this session`,
    onSelect: () => showOf(participant, 'privateNotes'),
  }] : [];

  /** Takes `participant` out of the channel - `leave` by one's own act, or `remove` it. */
  const takeOut = async (participant: ChannelParticipant, how: 'leave' | 'remove') => {
    const path = actorPath(participant.id);
    if (!path) return;
    try {
      await (how === 'leave' ? api.leaveChannel : api.removeParticipant)(sessionId, channelId, path);
      setChannel(prev => prev && {...prev, participants: prev.participants.filter(m => !sameActor(m.id, participant.id))});
    } catch (error) {
      reportError(`Failed to take ${participant.name} out of the channel`, error);
    }
  };

  useEffect(() => {
    const handleSnapshot = (data: string) => {
      try {
        const snapshot: ChannelSnapshot = JSON.parse(data);
        setLine(conversationOf(snapshot, nameIn(snapshot.channel), answersUnderWay(snapshot.channel.participants)));
        setChannel(snapshot.channel);
        setUsage(Object.fromEntries(snapshot.channel.participants.flatMap(m => m.usage ? [[actorKey(m.id), m.usage]] : [])));
        countedCalls.current = new Set(snapshot.items.flatMap(item => 'said' in item && item.said.id ? [item.said.id] : []));
        setNotes(snapshot.notes ?? []);
        setPendingApprovals(snapshot.pending_approvals ?? []);
        setPendingQuestions(snapshot.pending_questions ?? []);
        setAttachmentsVersion(v => v + 1);
      } catch (e) {
        reportError('Failed to read the channel state', e);
      }
    };

    // What an envelope changes of the channel as a whole: its participants, what calls used, the
    // workspace and the attachments, approvals and questions - of every thread.
    const handleChannelWide = (envelope: Envelope) => {
      const actorEvent = envelope.event;
      if (typeof actorEvent === 'string') return;
      const answerer = actorKey(envelope.context.sender_id);
      // What the sender's calls used, as the server sums them once its turn ended - also those
      // whose messages were not published.
      const readUsage = () => {
        const sender = envelope.context.sender_id;
        const agent = agentIdOf(sender) !== null ? actorPath(sender) : null;
        if (!agent) return;
        api.getParticipantUsage(sessionId, channelId, agent)
          .then(usage => { if (usage) setUsage(prev => ({...prev, [answerer]: usage})); })
          .catch(error => reportError('Failed to read what the agent used', error));
      };
      if (actorEvent.StateChanged) {
        // Where an agent stands now (issue 243), shown by its dot.
        const state = actorEvent.StateChanged;
        setChannel(prev => prev && {...prev, participants: prev.participants.map(m => actorKey(m.id) === answerer ? {...m, state} : m)});
      } else if (actorEvent.CompleteMessageGenerated || actorEvent.IntermediateMessageGenerated) {
        const said = (actorEvent.CompleteMessageGenerated ?? actorEvent.IntermediateMessageGenerated)!;
        // A tool call that ended may have changed the files.
        if (said.tool_results?.length) setWorkspaceVersion(v => v + 1);
        if (said.attachments?.length) setAttachmentsVersion(v => v + 1);
        // What its call used, at once: a plan's steps are many calls of one turn, whose end would
        // come too late for the gauge.
        if (said.id && !countedCalls.current.has(said.id)) {
          countedCalls.current.add(said.id);
          setUsage(prev => {
            const used = withCall(prev[answerer], said);
            return used ? {...prev, [answerer]: used} : prev;
          });
        }
      } else if (actorEvent.MessageModified || actorEvent.MessageRevoked || actorEvent.MessageReclassified) {
        setAttachmentsVersion(v => v + 1);
      } else if (actorEvent.EventRefused) {
        // What the user sent that the actor it went to did not take (issue 567) - the approver an
        // answer it does not count.
        const refusedAnswer = answersByEvent.current[envelope.context.correlation_id];
        if (refusedAnswer) {
          setAnsweredApprovals(prev => prev.filter(answered => answered !== refusedAnswer));
          setAnsweredByRule(prev => Object.fromEntries(Object.entries(prev).filter(([answered]) => answered !== refusedAnswer)));
        }
      } else if (actorEvent.ApprovalAnswered) {
        // This user's own answer, as their actor shows it - on this page, in another of theirs, or
        // given by a rule of theirs (issue 13).
        const {id, decision} = actorEvent.ApprovalAnswered;
        if (userIdOf(envelope.context.sender_id) === user.id) {
          answersByEvent.current[envelope.context.event_id] = id;
          setAnsweredApprovals(prev => prev.includes(id) ? prev : [...prev, id]);
          if (decision.by_rule) setAnsweredByRule(prev => ({...prev, [id]: decision.by_rule!}));
        }
      } else if (actorEvent.ApprovalRequested) {
        // Asked again by an agent that resumed its turn (issue 172), or already in the snapshot:
        // shown once.
        const requested = actorEvent.ApprovalRequested;
        setPendingApprovals(prev => prev.some(req => req.id === requested.id) ? prev : [...prev, requested]);
      } else if (actorEvent.ApprovalGranted || actorEvent.ApprovalDenied || actorEvent.ApprovalTimedOut || actorEvent.ApprovalRequestWithdrawn) {
        // Over only once the approver says so - decided, or no longer asked for (issue 148).
        const ended = (actorEvent.ApprovalGranted ?? actorEvent.ApprovalDenied ?? actorEvent.ApprovalTimedOut ?? actorEvent.ApprovalRequestWithdrawn)!.id;
        if (isApprover(envelope.context.sender_id)) {
          setPendingApprovals(prev => prev.filter(req => req.id !== ended));
          setAnsweredApprovals(prev => prev.filter(id => id !== ended));
        }
      } else if (actorEvent.QuestionRequested) {
        // Asked again by an agent that resumed its turn, or already in the snapshot: shown once.
        const requested = actorEvent.QuestionRequested;
        setPendingQuestions(prev => prev.some(q => q.id === requested.id) ? prev : [...prev, requested]);
      } else if (actorEvent.QuestionClosed) {
        const closed = actorEvent.QuestionClosed.id;
        setPendingQuestions(prev => prev.filter(q => q.id !== closed));
        setAnsweredQuestions(prev => prev.filter(id => id !== closed));
      } else if (actorEvent.TurnFinished || actorEvent.TurnGivenUp) {
        readUsage();
      } else if (actorEvent.CommandExecuted) {
        // A command turn ends without `TurnFinished`, and may have compacted the context (issue
        // 405): what the agent uses is read again.
        if (actorEvent.CommandExecuted.turn_id) readUsage();
      } else if (actorEvent.ActivityStarted) {
        // What an agent does of its own accord (issue 284): its dot says what.
        setActivities(prev => ({...prev, [answerer]: actorEvent.ActivityStarted!.activity}));
      } else if (actorEvent.ActivityFinished) {
        // The context may have shrunk: what the agent uses is read again.
        readUsage();
      }
    };

    const handleMessage = (data: string) => {
      try {
        const envelope: Envelope = JSON.parse(data);
        handleChannelWide(envelope);
        // What is said in another thread is left out of the line's view (issue 557): nothing of it
        // enters the line, no answer of it is shown under way there.
        if (!isShownInLine(envelope, me)) return;
        setLine(prev => withEvent(prev, envelope));
        // A change of a message the channel did not let in is undone by what it holds (issue 304),
        // where the line shows the message.
        const rejected = typeof envelope.event === 'string' ? undefined : envelope.event.EventRejected;
        if (rejected?.message && ['MessageModified', 'MessageRevoked', 'MessageReclassified'].includes(rejected.rejected)) {
          api.getMessage(sessionId, channelId, rejected.message)
            .then(standing => setLine(prev => withReplaced(prev, standing)))
            .catch(error => reportError('Failed to read the message back', error));
        }
      } catch (e) {
        console.error('Failed to parse SSE event', e);
      }
    };


    // Not known yet: the channel is fetched first, and this runs again.
    if (takesPart === undefined) {
      let cancelled = false;
      api.getChannel(sessionId, channelId)
        .then(found => { if (!cancelled) setChannel(found); })
        .catch(error => reportError('Failed to load the channel', error));
      return () => { cancelled = true; };
    }
    // Only a participant follows a channel live, through their own actor
    // (issue 104); anyone else who may read it sees its history as it is.
    if (!takesPart) {
      let cancelled = false;
      // With the channel as it is now, which names whose turns ended.
      Promise.all([api.getChannel(sessionId, channelId), api.getHistory(sessionId, channelId)])
        .then(([found, page]) => { if (!cancelled) showLastPage(page, nameIn(found)); })
        .catch(error => reportError('Failed to load the messages', error));
      return () => { cancelled = true; };
    }
    // The stream itself delivers the state it starts from, so there is no
    // window between loading the history and subscribing in which a message
    // could be missed (issue 34).
    return openEventStream(api.channelEventsUrl(sessionId, channelId), {
      onSnapshot: handleSnapshot,
      onEvent: handleMessage,
    });
  }, [sessionId, channelId, takesPart, user.id, me]);

  // The newest message of someone else shown is what the user read (issue
  // 404), marked while the tab is visible: as the channel is opened, as
  // messages arrive and as the tab comes back. Never one of their own, which
  // is shown before the channel takes it in; nor a notice of the UI's, which
  // is in no history.
  const newestOfOthers = useMemo(() => shown.findLast(m =>
    m.id && (m.role === 'User' || m.role === 'Assistant') && isOthers(m, me)
    && !m.givenUp && !m.commandOutput && !m.activity && !m.chainCutOff && !m.nudged
  )?.id ?? null, [shown, me]);
  const marked = useRef<string | null>(null);
  useEffect(() => {
    if (!newestOfOthers) return;
    const mark = () => {
      const key = `${channelId}:${newestOfOthers}`;
      if (document.visibilityState !== 'visible' || marked.current === key) return;
      marked.current = key;
      markRead(sessionId, channelId, newestOfOthers).catch(error => {
        marked.current = null;
        // A message the history does not hold marks nothing.
        if (!(error instanceof ApiRequestError && error.status === 404)) reportError('Failed to mark the channel read', error);
      });
    };
    mark();
    document.addEventListener('visibilitychange', mark);
    return () => document.removeEventListener('visibilitychange', mark);
  }, [sessionId, channelId, newestOfOthers]);

  /** Puts the page of the history before those shown in front of them. */
  const loadEarlier = async () => {
    if (line.earlier === null) return;
    try {
      const page = await api.getHistory(sessionId, channelId, line.earlier);
      setLine(prev => withEarlierPage(prev, page, nameIn(channel)));
    } catch (error) {
      reportError('Failed to load earlier messages', error);
    }
  };

  /**
   * Sends `input` into the line: a command, whose answer for the user alone is shown at once - one
   * for the whole channel arrives on the stream -, or a message.
   */
  const send = async (input: UserInput, attached: string[], release: Classification | null) => {
    if (input.kind === 'command') {
      const reply = await api.executeCommand(sessionId, channelId, input.line);
      if (reply.output !== null) {
        setLine(prev => withAdded(prev, {
          role: 'System',
          content: reply.output,
          commandOutput: {command: reply.command, private: true},
          // Private output has no envelope: the time it arrived.
          timestamp: new Date().toISOString(),
        }));
      }
    } else {
      await api.sendMessage(sessionId, channelId, input.text, attached, release ?? undefined);
    }
  };

  const handleStopGeneration = async () => {
    try {
      await api.cancelChannel(sessionId, channelId);
    } catch (error) {
      reportError('Failed to cancel generation', error);
    }
  };

  /** How a path of the REST API names the participant `sender` (an `actorKey`): `actorPath`. */
  const participantPath = (sender: string) => actorPath(participants.find(m => actorKey(m.id) === sender)?.id);

  /** Stops the answer of the agent `sender` alone (issue 235); the others go on. */
  const stopAnswer = async (sender: string) => {
    const path = participantPath(sender);
    if (!path) return;
    try {
      await api.cancelParticipant(sessionId, channelId, path);
    } catch (error) {
      reportError('Failed to stop the answer', error);
    }
  };

  /** Interrupts the call `call` of `msg` alone; the agent's turn goes on (issue 276). */
  const interruptCall = async (msg: Message, call: ToolCall) => {
    const path = msg.sender_id ? participantPath(actorKey(msg.sender_id)) : undefined;
    if (!path || !msg.turn_id || !call.id) return;
    try {
      await api.interruptToolCall(sessionId, channelId, path, msg.turn_id, call.id);
    } catch (error) {
      reportError('Failed to interrupt the tool call', error);
    }
  };

  // The commands ended in the channel: by a result that shows them so, or by the user (issue 294).
  const ended = useMemo(() => {
    const ids = endedCommands(merged);
    if (killedCommands?.channelId === channelId) killedCommands.ids.forEach(id => ids.add(id));
    return ids;
  }, [merged, killedCommands, channelId]);
  // What the messages of each turn carry, which its calls' cards leave to them (issue 535).
  const digestsByTurn = useMemo(() => attachedByTurn(shown), [shown]);

  /**
   * Ends the command `commandId` a call handed back as running, with what it
   * started (issue 294): offered to who may change the workspace's files.
   * The agent learns it the next time it follows the command. One the
   * workspace no longer knows is not offered again either.
   */
  const killCommand = async (commandId: string) => {
    const done = () => setKilledCommands(prev => ({channelId, ids: new Set([...(prev?.channelId === channelId ? prev.ids : []), commandId])}));
    try {
      await api.killCommand(sessionId, channelId, commandId);
      done();
      setWorkspaceVersion(v => v + 1);
    } catch (error) {
      if (error instanceof ApiRequestError && error.code === 'command_not_found') done();
      reportError('Failed to end the command', error);
    }
  };

  // What the messages of the chat need of the channel (issue 536).
  const shownChannel: ShownChannel = {
    attachments: channelAttachments,
    files: workspaceFiles ?? undefined,
    filesVersion: workspaceVersion,
    artifacts: channelArtifacts,
    onSelectPage,
    onKillCommand: workspace?.can_write ? (commandId) => void killCommand(commandId) : undefined,
    endedCommands: ended,
    digestsByTurn,
  };

  /**
   * Asks for another answer instead of `answer`, still under way (issue
   * 235): offered once its turn is known and where its answerer's
   * `can_retry` allows it (issue 242). Rejected if the answer ended
   * meanwhile; the retry icon on its last message is then the way.
   */
  const retryAnswer = async (answer: Answer) => {
    const path = participantPath(answer.sender);
    if (!path || !answer.turnId) return;
    try {
      await api.retryAnswer(sessionId, channelId, path, answer.turnId);
    } catch (error) {
      reportError('Failed to ask for another answer', error);
    }
  };

  // Only those questions are put to answer them (issue 165); everybody else
  // they reach sees what the agent waits for.
  const askedOf = (request: QuestionRequest) =>
    request.questions.ask.flatMap(a => typeof a === 'object' && 'actor' in a ? [a.actor] : []);
  const answerQuestions = async (request: QuestionRequest, responses: QuestionResponse[] | null) => {
    try {
      await api.answerQuestions(sessionId, channelId, request.id, responses);
      setAnsweredQuestions(prev => [...prev, request.id]);
    } catch (e) {
      reportError(responses ? 'Failed to answer the questions' : 'Failed to decline the questions', e);
    }
  };

  // Only who may approve the agent's calls is asked (issue 14); everybody
  // else sees what it waits for.
  const mayApprove = (approval: ApprovalRequest) =>
    participants.some(m => agentIdOf(m.id) === approval.agent_id && m.can_approve);

  // The answer first - the person decides this call now - then the rule
  // they chose to derive from it (issue 13); if the rule fails, the answer
  // stands.
  const handleApproval = async (approval: ApprovalRequest, approved: boolean) => {
    try {
      await api.respondToApproval(sessionId, channelId, approval.id, approved);
      setAnsweredApprovals(prev => [...prev, approval.id]);
    } catch (e) {
      reportError('Failed to respond to approval', e);
      return;
    }
    const choice = ruleChoices[approval.id];
    if (choice) {
      try {
        const added = await api.addApprovalRule(ruleOf(choice, approved, approval));
        setMyRules(prev => prev.some(r => r.id === added.id) ? prev : [...prev, added]);
      } catch (e) {
        reportError('The call was answered, but the rule could not be added', e);
      }
    }
  };

  const braveMode = approvingEverything(myRules, sessionId, channelId);
  const startBraveMode = async () => {
    try {
      const added = await api.addApprovalRule(approveEverything(sessionId, 60));
      setMyRules(prev => [...prev, added]);
    } catch (e) {
      reportError('Failed to approve everything in this session', e);
    }
  };
  const stopBraveMode = async (rule: ApprovalRule) => {
    try {
      if (rule.id) await api.removeApprovalRule(rule.id);
      setMyRules(prev => prev.filter(r => r.id !== rule.id));
    } catch (e) {
      reportError('Failed to stop approving everything', e);
    }
  };

  // What the parts of the chat are given of the channel (issue 595).
  const chatChannel: ChatChannel = {
    me,
    channel,
    participants,
    takesPart: !!takesPart,
    compartments,
    commands,
    attachments: channelAttachments,
    retry,
    retryAnswer,
    retryGivenUp,
    modify,
    revoke,
    reclassify: (message) => setReclassifying({kind: 'message', message}),
    showContext: (message) => setContextOf({channelId, message}),
    interruptCall,
    stopAnswer,
  };

  return (
    <ChatChannelContext value={chatChannel}>
      <div className="flex-1 flex overflow-hidden bg-gray-50 dark:bg-gray-800">
        <ShownChannelContext value={shownChannel}>
          <ConversationView
            key={channelId}
            conversation={line}
            onLoadEarlier={loadEarlier}
            newSince={newSince?.channelId === channelId ? newSince : undefined}
            draftKey={channelId}
            onSend={send}
            onStop={() => void handleStopGeneration()}
            cannotWrite={takesPart === false
              ? 'You do not take part in this channel: its history is shown as it is, not followed live, and you cannot write here.'
              : undefined}
          >
            {braveMode && (
              <div role="status" className="px-4 py-2 bg-amber-100 dark:bg-amber-900/30 border-t border-amber-300 dark:border-amber-800 text-xs text-amber-900 dark:text-amber-200 flex items-center gap-2">
                <ShieldAlert size={14} />
                <span className="flex-1">
                  Your rule approves every tool call here{braveMode.expires_at ? ` until ${new Date(braveMode.expires_at).toLocaleTimeString()}` : ''} - except where a rule of the session or channel asks.
                </span>
                <button onClick={() => void stopBraveMode(braveMode)} className="rounded-sm border border-amber-400 px-2 py-0.5 hover:bg-amber-200 dark:hover:bg-amber-800">
                  Stop
                </button>
              </div>
            )}
            {pendingQuestions.length > 0 && (
              <div className="p-4 bg-blue-50 dark:bg-blue-900/20 border-t border-blue-200 dark:border-blue-900/50 space-y-4">
                <h4 className="text-sm font-semibold text-blue-800 dark:text-blue-200 flex items-center gap-2">
                  <MessageCircleQuestion size={16} />
                  Questions of an agent
                </h4>
                {pendingQuestions.map(request => (
                  <QuestionCard
                    key={request.id}
                    request={request}
                    asked={askedOf(request).some(id => sameActor(id, me))}
                    askedNames={askedOf(request).flatMap(id => participants.find(m => sameActor(m.id, id))?.name ?? [])}
                    answered={answeredQuestions.includes(request.id)}
                    onAnswer={responses => answerQuestions(request, responses)}
                  />
                ))}
              </div>
            )}
            {pendingApprovals.length > 0 && (
              <div className="p-4 bg-yellow-50 dark:bg-yellow-900/20 border-t border-yellow-200 dark:border-yellow-900/50 space-y-4">
                <h4 className="text-sm font-semibold text-yellow-800 dark:text-yellow-200 flex items-center gap-2">
                  <Wrench size={16} />
                  <span className="flex-1">Tool Execution Approval Required</span>
                  {!braveMode && pendingApprovals.some(mayApprove) && (
                    <button
                      onClick={() => void startBraveMode()}
                      className="text-xs font-normal text-yellow-800 dark:text-yellow-200 hover:underline"
                      title="A rule of yours that approves every tool call of the agents you are responsible for, in this session, for an hour - shown for as long as it lasts"
                    >
                      Approve everything in this session for an hour
                    </button>
                  )}
                </h4>
                {pendingApprovals.map(approval => (
                  <div key={approval.id} className="bg-white dark:bg-gray-800 rounded-lg border border-yellow-200 dark:border-yellow-900/50 shadow-xs p-4">
                    <div className="flex items-center justify-between mb-2">
                       <div className="text-xs font-mono font-bold text-gray-500">{approval.tool_call.function_name}</div>
                       <div className="text-[11px] text-gray-400">ID: {approval.id}</div>
                    </div>
                    {approval.tool_call.description && (
                      <p className="text-sm whitespace-pre-wrap mb-2">{approval.tool_call.description}</p>
                    )}
                    <pre className="text-xs font-mono bg-gray-50 dark:bg-gray-900 p-2 rounded-sm mb-4 overflow-x-auto">
                      {formatArguments(approval.tool_call.arguments)}
                    </pre>
                    {approval.explicit && (
                      <p className="text-xs text-yellow-800 dark:text-yellow-200 mb-3 flex items-center gap-1">
                        <ShieldAlert size={12} />
                        A rule of this session or channel asks for this call to be decided by a person, each time.
                      </p>
                    )}
                    {answeredApprovals.includes(approval.id) ? (
                      <p className="text-sm text-gray-500 dark:text-gray-400 text-center">
                        {answeredByRule[approval.id] ? 'Answered by your rule' : 'Answered'} - waiting for the decision.
                      </p>
                    ) : !mayApprove(approval) ? (
                      <p className="text-sm text-gray-500 dark:text-gray-400 text-center">Waiting for whoever is responsible for the agent.</p>
                    ) : (
                    <>
                    {!approval.explicit && approval.suggested && (
                      <ApprovalRuleOffer
                        choice={ruleChoices[approval.id] ?? null}
                        initial={initialChoice(approval.suggested)}
                        onChange={(choice) => setRuleChoices(prev => ({...prev, [approval.id]: choice}))}
                      />
                    )}
                    <div className="flex gap-2">
                      <button
                        onClick={() => handleApproval(approval, true)}
                        className="flex-1 bg-green-600 hover:bg-green-700 text-white text-sm py-2 rounded-md transition-colors"
                      >
                        Approve
                      </button>
                      <button
                        onClick={() => handleApproval(approval, false)}
                        className="flex-1 bg-red-600 hover:bg-red-700 text-white text-sm py-2 rounded-md transition-colors"
                      >
                        Deny
                      </button>
                    </div>
                    </>
                    )}
                  </div>
                ))}
              </div>
            )}
          </ConversationView>
        </ShownChannelContext>

        {isAddingParticipant && channel && (
          <AddParticipantDialog
            sessionId={sessionId}
            channel={channel}
            onAdded={(updated) => { setChannel(updated); setIsAddingParticipant(false); }}
            onClose={() => setIsAddingParticipant(false)}
          />
        )}
        {isDescribing && channel && (
          <DescriptionDialog
            title={`What ${channel.name} is for`}
            place="channel"
            summary={channel.summary}
            briefing={channel.briefing}
            canChange={channel.can_change}
            onSubmit={async (description) => setChannel(await api.setChannelDescription(sessionId, channelId, description))}
            onClose={() => setIsDescribing(false)}
          />
        )}
        {isChangingLanguage && channel && (
          <LanguageDialog
            title={`Language of ${channel.name}`}
            current={channel.language}
            empty="The session's"
            emptyHint="empty: the session's, if it has one"
            onSubmit={async (language) => setChannel(await api.setChannelLanguage(sessionId, channelId, language))}
            onClose={() => setIsChangingLanguage(false)}
          />
        )}
        {isShowingRules && channel && (
          <ApprovalRulesDialog
            title={`Approval rules of ${channel.name}`}
            place="channel"
            current={channel.approval_rules}
            canChange={channel.can_change}
            onSubmit={async (rules) => setChannel(await api.setChannelApprovalRules(sessionId, channelId, rules))}
            onClose={() => setIsShowingRules(false)}
          />
        )}
        {reclassifying && channel && (reclassifying.kind === 'channel' ? (
          <ClassificationDialog
            title={`Classification of ${channel.name}`}
            current={channel.classification}
            levels={CONFIDENTIALITY_LEVELS}
            compartments={compartments}
            releaseWarning="This releases the channel: everything in it - its history, and what its agents know - becomes readable by the wider circle. What was read under the old classification is not retracted by raising it again. Answers the agents are writing right now are discarded."
            onSubmit={async (c) => setChannel(await api.reclassifyChannel(sessionId, channelId, c))}
            onClose={() => setReclassifying(null)}
          />
        ) : reclassifying.message.classification && (
          <ClassificationDialog
            title="Classification of the message"
            current={reclassifying.message.classification}
            levels={levelsUpTo(channel.classification.level)}
            compartments={compartments}
            releaseWarning="This releases the message: it becomes readable by the wider circle."
            onSubmit={(c) => api.reclassifyMessage(sessionId, channelId, reclassifying.message.id!, c)}
            onClose={() => setReclassifying(null)}
          />
        ))}
        {detailShown ? (
          <>
          <ResizeHandle panel={detailPanel} panelSide="right" label="Resize the panel" />
          <div className="shrink-0 max-w-full flex min-h-0" style={{width: detailPanel.width}}>
          {contextOf && contextOf.channelId === channelId ? (
          <ContextPanel
            key={contextOf.message.id}
            sessionId={sessionId}
            channelId={channelId}
            attachments={channelAttachments}
            message={contextOf.message}
            me={me}
            participants={participants}
            onClose={() => setContextOf(null)}
          />
        ) : historyOf && historyOf.channelId === channelId ? (
          <HistoryPanel
            key={actorKey(historyOf.participant.id)}
            sessionId={sessionId}
            channelId={channelId}
            attachments={channelAttachments}
            participant={historyOf.participant}
            me={me}
            participants={participants}
            version={line.messages.length}
            onShowContext={message => setContextOf({channelId, message})}
            onClose={() => setHistoryOf(null)}
          />
        ) : participantPanel && participantPanel.channelId === channelId && participantPanel.shows === 'tools' ? (
          <ToolsPanel
            key={actorKey(participantPanel.participant.id)}
            sessionId={sessionId}
            channelId={channelId}
            participant={participantPanel.participant}
            onClose={() => setParticipantPanel(null)}
          />
        ) : participantPanel && participantPanel.channelId === channelId && participantPanel.shows === 'privateNotes' ? (
          <PrivateNotesPanel
            key={actorKey(participantPanel.participant.id)}
            sessionId={sessionId}
            channelId={channelId}
            participant={participantPanel.participant}
            onClose={() => setParticipantPanel(null)}
          />
        ) : participantPanel && participantPanel.channelId === channelId && participantPanel.shows === 'profile' ? (
          <ProfilePanel
            key={actorKey(participantPanel.participant.id)}
            sessionId={sessionId}
            channelId={channelId}
            participant={participantPanel.participant}
            onClose={() => setParticipantPanel(null)}
          />
          ) : null}
          </div>
          </>
        ) : (
        <>
        <ResizeHandle panel={channelPanel} panelSide="right" label="Resize the channel panel" className="hidden md:block" />
        <div className="shrink-0 border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 overflow-y-auto hidden md:block" style={{width: channelPanel.width}}>
          <div className="p-4">
            <div className="flex flex-wrap items-center gap-1 mb-4 text-gray-500 dark:text-gray-400" role="tablist">
              {SIDE_TABS.map(({id, icon: Icon, label}) => (
                <button
                  key={id}
                  role="tab"
                  aria-selected={sideTab === id}
                  onClick={() => setSideTab(id)}
                  className={cn("flex items-center gap-1.5 px-2 py-1 rounded-md", sideTab === id && "bg-gray-100 dark:bg-gray-800 text-gray-800 dark:text-gray-100")}
                >
                  <Icon size={16} />
                  <h3 className="font-semibold uppercase text-xs tracking-wider">{label}</h3>
                </button>
              ))}
            </div>
            {sideTab === 'notes' ? <NotesList notes={notes} /> : sideTab === 'workspace' ? (
              <ChannelWorkspacePanel workspace={workspace} files={workspaceFiles} version={workspaceVersion} />
            ) : sideTab === 'attachments' ? (
              <ChannelAttachmentsPanel key={channelId} attachments={channelAttachments} version={attachmentsVersion} />
            ) : (<>
            {channel && (
              <ChannelDescription channel={channel} onEdit={() => setIsDescribing(true)} />
            )}
            {channel && (
              <div
                className="flex items-center gap-2 mb-4 text-xs text-gray-500 dark:text-gray-400"
                title={responderPolicyInfo(channel.responder_policy).description}
              >
                <Scale size={14} />
                <span>Moderator:</span>
                <span className="text-gray-700 dark:text-gray-300">{responderPolicyInfo(channel.responder_policy).label}</span>
              </div>
            )}
            {channel && (
              <div
                className="flex items-center gap-2 mb-4 -mt-2 text-xs text-gray-500 dark:text-gray-400"
                title={classificationDescription(channel.classification)}
              >
                <Shield size={14} />
                <span>Classification:</span>
                <span className="text-gray-700 dark:text-gray-300">{classificationLabel(channel.classification)}</span>
                {channel.can_change && (
                  <button
                    onClick={() => setReclassifying({kind: 'channel'})}
                    className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
                    title="Change the classification"
                    aria-label="Change the classification of the channel"
                  >
                    <Pencil size={12} />
                  </button>
                )}
              </div>
            )}
            {channel && (
              <div
                className="flex items-center gap-2 mb-4 -mt-2 text-xs text-gray-500 dark:text-gray-400"
                title="The language the agents are told to write in. Nothing is translated."
              >
                <Languages size={14} />
                <span>Language:</span>
                <span className="text-gray-700 dark:text-gray-300">{languageLabel(channel.effective_language)}</span>
                {channel.language === null && channel.effective_language !== null && <span>(the session's)</span>}
                {channel.can_change && (
                  <button
                    onClick={() => setIsChangingLanguage(true)}
                    className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
                    title="Change the language"
                    aria-label="Change the language of the channel"
                  >
                    <Pencil size={12} />
                  </button>
                )}
              </div>
            )}
            {channel && (
              <div
                className="flex items-center gap-2 mb-4 -mt-2 text-xs text-gray-500 dark:text-gray-400"
                title="Whether its participants work on the session's workspace here - where it and the channel are classified alike."
              >
                <HardDrive size={14} />
                <span>Workspace:</span>
                {channel.can_change ? (
                  <label className="flex items-center gap-1 text-gray-700 dark:text-gray-300">
                    <input
                      type="checkbox"
                      checked={channel.uses_workspace}
                      onChange={(e) => {
                        const uses = e.target.checked;
                        api.setChannelUsesWorkspace(sessionId, channelId, uses)
                          .then(updated => { setChannel(updated); setWorkspaceVersion(v => v + 1); })
                          .catch(error => reportError('Failed to change the channel\'s use of the workspace', error));
                      }}
                    />
                    used here
                  </label>
                ) : (
                  <span className="text-gray-700 dark:text-gray-300">{channel.uses_workspace ? 'used here' : 'not used here'}</span>
                )}
              </div>
            )}
            {channel && (channel.can_change || channel.approval_rules.length > 0) && (
              <div
                className="flex items-center gap-2 mb-4 -mt-2 text-xs text-gray-500 dark:text-gray-400"
                title="What the channel restricts of its agents' tool calls: asked for or denied, whatever the tool says."
              >
                <ShieldAlert size={14} />
                <span>Approval rules:</span>
                <button
                  onClick={() => setIsShowingRules(true)}
                  className="text-gray-700 dark:text-gray-300 hover:underline"
                  aria-label={channel.can_change ? 'Change the approval rules of the channel' : 'Show the approval rules of the channel'}
                >
                  {channel.approval_rules.length === 0 ? 'none' : `${channel.approval_rules.length}`}
                </button>
              </div>
            )}
            {channelUsage && (
              <HoverCard
                className="flex items-center gap-2 mb-4 -mt-2 text-xs text-gray-500 dark:text-gray-400"
                content={<UsageStats groups={[{
                  title: `Usage of all ${formatTokens(channelUsage.calls)} model calls of the agents shown`,
                  rows: [
                    ['Total input tokens', formatTokens(channelUsage.prompt)],
                    ['Total output tokens', formatTokens(channelUsage.completion)],
                  ],
                }]} />}
              >
                <Gauge size={14} />
                <span>Total tokens:</span>
                <span className="text-gray-700 dark:text-gray-300">{formatTokensShort(channelUsage.prompt + channelUsage.completion)}</span>
              </HoverCard>
            )}
            <div className="space-y-2">
              {participants.map(participant => {
                const dot = stateDot(participant, activities[actorKey(participant.id)]);
                const stoppable = participant.can_cancel && participant.state !== undefined && INTERRUPTIBLE.includes(participant.state);
                return (
                  <div key={actorKey(participant.id)} className="group flex items-center space-x-2 p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
                    {dot
                      ? <div className={cn("w-2 h-2 rounded-full shrink-0", dot.className)} title={dot.title} role="img" aria-label={dot.title} />
                      : <div className="w-2 h-2 shrink-0" />}
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center space-x-2">
                        <span className="text-sm text-gray-700 dark:text-gray-300 truncate">{participant.name}</span>
                        {participant.handle && <span className="text-xs font-mono text-gray-400 dark:text-gray-500 truncate">@{participant.handle}</span>}
                      </div>
                      {usage[actorKey(participant.id)] && <TokenGauge usage={usage[actorKey(participant.id)]} />}
                    </div>
                    {stoppable && (
                      <button
                        type="button"
                        onClick={() => void stopAnswer(actorKey(participant.id))}
                        className="p-0.5 rounded-sm text-gray-400 hover:text-red-600 dark:hover:text-red-400 transition-colors"
                        title={`Stop ${participant.name}`}
                        aria-label={`Stop ${participant.name}`}
                      >
                        <Square size={12} fill="currentColor" />
                      </button>
                    )}
                    <ActionMenu
                      label={`Options of ${participant.name}`}
                      className={ROW_MENU}
                      actions={participantActions(participant)}
                    />
                  </div>
                );
              })}
            </div>
            {channel?.can_change && (
              <button
                onClick={() => setIsAddingParticipant(true)}
                className="mt-3 flex items-center gap-2 px-2 py-1 rounded-md text-xs text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-800 hover:text-gray-800 dark:hover:text-gray-100"
              >
                <UserPlus size={14} />
                <span>Add participant</span>
              </button>
            )}
            {(channel?.former_participants?.length ?? 0) > 0 && (
              <div className="mt-4">
                <button
                  onClick={() => setShowFormer(shown => !shown)}
                  className="flex items-center gap-1 px-2 py-1 text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-100"
                  aria-expanded={showFormer}
                >
                  {showFormer ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                  <span>Former participants ({channel!.former_participants.length})</span>
                </button>
                {showFormer && (
                  <div className="space-y-2 mt-1">
                    {channel!.former_participants.map(participant => {
                      const left = formatTimestamp(participant.left?.at);
                      return (
                        <div key={actorKey(participant.id)} className="group flex items-center space-x-2 p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
                          <div className="w-2 h-2 rounded-full bg-gray-300 dark:bg-gray-600 shrink-0" />
                          <div className="flex-1 min-w-0">
                            <div className="text-sm text-gray-500 dark:text-gray-400 truncate">{participant.name}</div>
                            {left && <div className="text-[11px] text-gray-400 dark:text-gray-500" title={left.full}>left {left.short}</div>}
                          </div>
                          <ActionMenu
                            label={`Options of ${participant.name}`}
                            className={ROW_MENU}
                            actions={contextActions(participant)}
                          />
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            )}
            </>)}
          </div>
        </div>
        </>
        )}
      </div>
    </ChatChannelContext>
  );
};
