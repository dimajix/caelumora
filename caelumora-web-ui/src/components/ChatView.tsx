import React, {useEffect, useMemo, useRef, useState} from 'react';
import {
  AlertTriangle,
  Ban,
  BellRing,
  ChevronDown,
  ChevronRight,
  Cog,
  Gauge,
  HardDrive,
  History,
  IdCard,
  Languages,
  Lock,
  MessageCircleQuestion,
  MessageSquare,
  NotebookPen,
  Pencil,
  RotateCcw,
  Scale,
  Shield,
  ShieldAlert,
  ScrollText,
  Send,
  Square,
  Trash2,
  Undo2,
  UserMinus,
  Unlink,
  UserPlus,
  Wrench,
} from 'lucide-react';
import type {
  ActorId,
  ActorState,
  Rejection,
  RetryRefused,
  ApprovalRequest,
  ApprovalRule,
  ChannelParticipant,
  ChannelResponse,
  ChannelSnapshot,
  Command,
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
import {answersOnlyWhenMentioned, responderPolicyInfo} from '../moderator';
import {
  CONFIDENTIALITY_LEVELS,
  type Classification,
  classificationDescription,
  classificationLabel,
  levelsUpTo,
  sameClassification,
} from '../confidentiality';
import {ClassificationPicker} from './ClassificationPicker';
import {ClassificationDialog} from './ClassificationDialog';
import {MessageEditor} from './MessageEditor';
import {LanguageDialog} from './LanguageDialog';
import {ApprovalRulesDialog} from './ApprovalRulesDialog';
import {languageLabel} from '../language';
import {approvalRulesChangedChannel, changedChannel, descriptionChangedChannel, languageChangedChannel, joinedOrLeftChannel, onGlobalEvent, profileApprovalRules, reclassifiedChannel, saidInChannel} from '../globalEvents';
import {isOthers, markRead} from '../unread';
import {api} from '../api';
import type {Page} from '../page';
import {ApiRequestError, reportError} from '../errors';
import {cn} from '../classNames';
import {MarkdownMessage} from './MarkdownMessage';
import {GrowingTextarea} from './GrowingTextarea';
import {MessageFooter, MessageView, Reasoning} from './MessageView';
import {activityNotice, activityUnderWay, chatMessages, chatRows, commandNotice, endedCommands, formatArguments, givenUpNotice, formatTimestamp, formatTokens, formatTokensShort, isOwn, mayBeAnsweredAgain, mergeToolResults, participantNotice, planStepMark, rowTurn, turnGroups, type ChatRow} from '../messages';
import {PlanSteps} from './PlanSteps';
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
import {actorKey, actorUuid, GROUP_HANDLES, handleOf, mentionQueryAt} from '../mention';
import {commandQueryAt, parseInput} from '../command';
import {openEventStream} from '../stream';
import {loadDraft, saveDraft} from '../session';

/** The menu button of a participant's row: shown while the row is hovered or the button focused. */
const ROW_MENU = 'text-gray-400 opacity-0 group-hover:opacity-100 focus:opacity-100 hover:text-gray-700 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-700';

/** Why an agent did not answer again (issue 234), as this page words it. */
function retryRefusedText(reason: RetryRefused): string {
  switch (reason) {
    case 'not_its_answer': return 'that is not an answer of its own it could give again';
    case 'not_last': return 'more was said or done since that answer';
    case 'calls_ran': return 'tools that already ran would be taken back with it';
    case 'asked_again': return 'it was asked again in the meantime';
    case 'elsewhere': return 'it runs elsewhere and cannot take back what it was given';
    case 'ended': return 'its answer had already ended';
    default: return 'for a reason this page does not know';
  }
}

/** Why the channel did not let in what the user sent (issue 304), in words. */
function rejectionText(reason: Rejection): string {
  switch (reason) {
    case 'forbidden': return 'you may not do this here';
    case 'classification_refused': return 'it is classified so that the channel does not admit it';
    case 'message_not_found': return 'the message it is about is not in the channel';
    case 'message_unchangeable': return 'the message was taken back or cannot be changed so';
    case 'invalid': return 'it does not fit the channel as it is';
    default: return 'for a reason this page does not know';
  }
}

/** What the user sent that the channel did not let in (issue 304), by its kind. */
function rejectedText(kind: string): string {
  switch (kind) {
    case 'CompleteMessageGenerated': return 'Your message';
    case 'MessageModified': return 'Your change to a message';
    case 'MessageRevoked': return 'Taking a message back';
    case 'MessageReclassified': return 'The new classification of a message';
    case 'RetryRequested': return 'Your request for another answer';
    case 'QuestionAnswered': case 'QuestionDeclined': return 'Your answer to the questions';
    case 'CommandRequested': return 'Your command';
    case 'NudgeRequested': return 'Your request to respond';
    default: return 'What you sent';
  }
}

type MentionableParticipant = ChannelParticipant & {handle: string};

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
 * own accord, `activity`, as it said it last (issue 284). A user or an
 * approver has none.
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

/** An entry of the completion list: a participant or everyone to mention, or a command to run. */
// A group: a handle that mentions every participant (issue 401). A command
// suggested: one of the channel, or one an agent carries out (issue 269),
// with the handle it is written with - `all` for every agent that carries it
// out.
type Suggestion =
  | {kind: 'mention'; participant: MentionableParticipant}
  | {kind: 'group'; handle: string}
  | {kind: 'command'; command: Command; handle?: string};

/**
 * An agent's answer under way (issue 203): what of it has streamed in since
 * its last complete message, and the plan step it is at. Several agents can
 * answer at once, each with an answer of its own.
 */
interface Answer {
  /** Its sender, as `actorKey`. */
  sender: string;
  name: string | null;
  /**
   * The turn it is (issue 235): from `ResponseStarted`, or - for an answer
   * that began before the page was loaded - from its first message.
   */
  turnId: string | null;
  reasoning: string;
  content: string;
  /** Whether what streams in is a step of a plan (issue 263), shown in the step's block. */
  internal: boolean;
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
 * `answers` with the answer of `sender` changed by `change` - added at the
 * end if it has none, so that answers keep the order they began in.
 */
/**
 * Whose name an actor has in `channel` - a participant's, or one's that left
 * (issue 164) - to name whose turn was given up (issue 258).
 */
function nameIn(channel: ChannelResponse | null): (actor: ActorId) => string | undefined {
  return actor => [...(channel?.participants ?? []), ...(channel?.former_participants ?? [])]
    .find(m => actorKey(m.id) === actorKey(actor))?.name;
}

function withAnswer(answers: Answer[], sender: string, name: string | null, change: (answer: Answer) => Answer): Answer[] {
  const index = answers.findIndex(a => a.sender === sender);
  if (index < 0) {
    return [...answers, change({sender, name, turnId: null, reasoning: '', content: '', internal: false})];
  }
  return answers.map((a, i) => i === index ? change({...a, name: name ?? a.name}) : a);
}

interface ChatViewProps {
  sessionId: string;
  channelId: string;
  /** Who is logged in: only a participant follows the channel live. */
  user: UserResponse;
  /** Opens a page in the place of the channel - a skill's, for a call that names one (issue 281). */
  onSelectPage: (page: Page) => void;
}

export const ChatView: React.FC<ChatViewProps> = ({ sessionId, channelId, user, onSelectPage }) => {
  // The user's actor, whose messages are their own (issue 407).
  const me = useMemo<ActorId>(() => ({User: user.id}), [user.id]);
  const [messages, setMessages] = useState<Message[]>([]);
  const [channel, setChannel] = useState<ChannelResponse | null>(null);
  const [notes, setNotes] = useState<PublishedNotes[]>([]);
  const [sideTab, setSideTab] = useState<'channel' | 'notes' | 'workspace'>('channel');
  // Changes whenever the session's workspace or its files may have changed
  // here: a tool call ended, the channel's switch was set (issue 253).
  const [workspaceVersion, setWorkspaceVersion] = useState(0);
  // The workspace's files as this user may read them (issue 23): what a call offers is downloaded from them (issue 273).
  const workspaceId = channel?.workspace_id;
  const workspaceFiles = useMemo(() => workspaceId ? api.workspaceFiles(workspaceId) : null, [workspaceId]);
  // The workspace itself, and for which: `null` where it could not be fetched.
  const [fetchedWorkspace, setFetchedWorkspace] = useState<{id: string; workspace: WorkspaceResponse | null} | null>(null);
  // `undefined` while it is fetched, `null` where there is none to use.
  const workspace = !workspaceId ? null : fetchedWorkspace?.id === workspaceId ? fetchedWorkspace.workspace : undefined;
  // The commands the user ended here (issue 294), until a result shows them ended.
  const [killedCommands, setKilledCommands] = useState<{channelId: string; ids: ReadonlySet<string>} | null>(null);
  // The channel's artifacts as this user reaches them: what a call published is downloaded from them (issue 271).
  const channelArtifacts = useMemo(() => api.channelArtifacts(sessionId, channelId), [sessionId, channelId]);
  // The widths of the channel panel and of the panel showing an agent's
  // context, history or tools (issue 231). The channel panel keeps the key
  // it had as the participants panel (issue 245), so that its width stays.
  const channelPanel = usePanelWidth('participants');
  const detailPanel = usePanelWidth('detail');
  // Restored from session storage, so a forced re-login does not lose it.
  const [inputText, setInputText] = useState(() => loadDraft(channelId));
  const [isSending, setIsSending] = useState(false);
  // How the next message is classified, in the channel it was chosen in;
  // `null`: as the channel (issue 133).
  const [chosenRelease, setChosenRelease] = useState<{channelId: string; classification: Classification} | null>(null);
  const release = chosenRelease?.channelId === channelId ? chosenRelease.classification : null;
  const setRelease = (classification: Classification | null) =>
    setChosenRelease(classification && {channelId, classification});
  const [compartments, setCompartments] = useState<Compartment[]>([]);
  // The answers under way, in the order they began (issue 203). Shown below
  // every complete message: a message joins the history when it is complete,
  // so an answer that ends first is shown first, as a reload shows it.
  const [answers, setAnswers] = useState<Answer[]>([]);
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
  const isGenerating = answers.length > 0;
  const [pendingApprovals, setPendingApprovals] = useState<ApprovalRequest[]>([]);
  // The pending approvals this user answered, still waiting for the decision.
  const [answeredApprovals, setAnsweredApprovals] = useState<string[]>([]);
  // Of those, the ones a rule of this user's answered (issue 13), by id.
  const [answeredByRule, setAnsweredByRule] = useState<Record<string, string>>({});
  // The rule each card is to add with its answer, by request id.
  const [ruleChoices, setRuleChoices] = useState<Record<string, RuleChoice | null>>({});
  // The questions the agents' calls wait for the answers to (issue 165),
  // and those this user answered, until the agent closes them.
  const [pendingQuestions, setPendingQuestions] = useState<QuestionRequest[]>([]);
  const [answeredQuestions, setAnsweredQuestions] = useState<string[]>([]);
  // This user's own rules (issue 13) - for the brave mode, shown while it
  // lasts; they follow the global stream.
  const [myRules, setMyRules] = useState<ApprovalRule[]>([]);
  // The snapshot holds the last page of the history; the pages before it on
  // request, read before `earlier` (issue 218) - `null` once there are none.
  const [earlier, setEarlier] = useState<number | null>(null);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  // Where the new messages began as the channel was opened (issue 404):
  // after the message last read, as the server said then, so that the line
  // stays where it was while the mark moves on as the user reads.
  const [newSince, setNewSince] = useState<{channelId: string; after: string | null; count: number} | null>(null);
  if (channel?.id === channelId && newSince?.channelId !== channelId) {
    setNewSince({channelId, after: channel.unread?.read_up_to ?? null, count: channel.unread?.messages ?? 0});
  }

  /**
   * Shows `page`, the last page of the history, in place of what is shown,
   * as a snapshot would (issue 218): the pages before it that were loaded
   * are let go, and what arrived after it was read is kept.
   */
  const showLastPage = (page: HistoryPage<Message>, nameOf: (actor: ActorId) => string | undefined) => {
    const data = chatMessages(page.items, nameOf);
    const known = new Set(data.flatMap(m => m.id ? [m.id] : []));
    const read = data[data.length - 1]?.timestamp;
    setMessages(prev => [...data, ...prev.filter(m => m.id && !known.has(m.id) && read && m.timestamp && m.timestamp > read)]);
    setEarlier(page.earlier);
  };

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // `@`-completion: the caret position the suggestions were computed for, the
  // highlighted suggestion, and the query the user dismissed with Escape.
  const [caret, setCaret] = useState(0);
  const [suggestionIndex, setSuggestionIndex] = useState(0);
  const [dismissedMention, setDismissedMention] = useState<number | null>(null);
  // The agent message whose context is shown, with the channel it belongs
  // to - another channel shows none (issue 41).
  const [contextOf, setContextOf] = useState<{channelId: string; message: Message} | null>(null);
  // The user's own message being edited, and the one whose taking back
  // waits for a second click (issue 234).
  const [editing, setEditing] = useState<string | null>(null);
  // The turn whose messages are pointed at, shown together (issue 415).
  const [pointedTurn, setPointedTurn] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<string | null>(null);
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
  const [commands, setCommands] = useState<Command[]>([]);
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
  const mentionQuery = mentionQueryAt(inputText, caret);
  const commandQuery = commandQueryAt(inputText, caret);
  // Where the completed token starts; a command always starts at 0.
  const suggestionStart = commandQuery !== null ? 0 : mentionQuery?.start ?? null;
  // The commands the agents here carry out (issue 269), by name, with the
  // handles of the agents that do: one is suggested for each of them, and
  // for `@all` where several do.
  const agentCommands = new Map<string, {command: Command; handles: string[]}>();
  for (const participant of participants) {
    if (!participant.handle || participant.left) continue;
    for (const command of participant.commands ?? []) {
      const entry = agentCommands.get(command.name) ?? {command, handles: []};
      entry.handles.push(participant.handle);
      agentCommands.set(command.name, entry);
    }
  }
  const agentSuggestions = [...agentCommands.values()].flatMap(({command, handles}) =>
    [...handles, ...(handles.length > 1 ? [GROUP_HANDLES[0]] : [])].map((handle): Suggestion => ({kind: 'command', command, handle})));
  const mentionText = (mentionQuery?.query ?? '').toLowerCase();
  const suggestions: Suggestion[] = suggestionStart === null || suggestionStart === dismissedMention
    ? []
    : commandQuery !== null
      ? [...commands.map((command): Suggestion => ({kind: 'command', command})), ...agentSuggestions]
          .filter(s => s.kind === 'command' && s.command.name.startsWith(commandQuery.toLowerCase()))
      : [
          ...participants
            .filter((m): m is MentionableParticipant =>
              !!m.handle && (m.handle.startsWith(handleOf(mentionText)) || m.name.toLowerCase().includes(mentionText)))
            .map((participant): Suggestion => ({kind: 'mention', participant})),
          ...GROUP_HANDLES
            .filter(handle => handle.startsWith(handleOf(mentionText)))
            .map((handle): Suggestion => ({kind: 'group', handle})),
        ];

  // Whether the logged-in user takes part in this channel; `undefined` until
  // the channel is known. Joining or leaving changes it (the participants follow
  // the global stream), and the view follows.
  const takesPart = channel?.id === channelId
    ? channel.participants.some(m => typeof m.id !== 'string' && m.id.User === user.id)
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
  // longer fetch it and keeps what they saw until they leave the channel.
  useEffect(() => {
    return onGlobalEvent(event => {
      if (joinedOrLeftChannel(event) !== channelId) return;
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
      if (!said || said.channelId !== channelId) return;
      setMessages(prev => prev.some(m => m.id === said.message.id) ? prev : [...prev, said.message]);
    });
  }, [channelId, takesPart]);

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
      const language = languageChangedChannel(event);
      if (language) {
        if (language.channelId === channelId) setChannel(prev => prev && {...prev, language: language.language});
        return;
      }
      const reclassified = reclassifiedChannel(event);
      if (!reclassified || reclassified.channelId !== channelId) return;
      setChannel(prev => prev && {...prev, classification: reclassified.classification});
      // Its last page anew, as a snapshot would show it (issue 218).
      Promise.all([api.getChannel(sessionId, channelId), api.getHistory(sessionId, channelId)])
        .then(([found, page]) => showLastPage(page, nameIn(found)))
        .catch(error => reportError('Failed to load the messages', error));
    });
  }, [sessionId, channelId]);

  /** Whether the user may change the classification of `msg`: its author, or the channel's owner. */
  const mayReclassify = (msg: Message) =>
    !!msg.id && !!msg.classification && (!!channel?.can_change || isOwn(msg, me));

  /**
   * Whether `msg` is the user's own to edit and take back (issue 234): a
   * stored message of theirs not taken back, in a channel they take part in
   * - only a participant has an actor to change it through.
   */
  const mayChange = (msg: Message) =>
    !!takesPart && !!msg.id && !msg.revoked && isOwn(msg, me);

  const saveEdit = async (msg: Message, content: string) => {
    try {
      await api.modifyMessage(sessionId, channelId, msg.id!, content);
      setEditing(null);
    } catch (error) {
      reportError('Failed to edit the message', error);
    }
  };

  /** Whether the user may ask `sender` to answer again: a participant whose `can_retry` allows it (issue 242). */
  const mayRetryOf = (sender: string) => participants.some(m => actorKey(m.id) === sender && m.can_retry);

  /**
   * Whether the user may ask the agent that said `msg` to answer again (issue
   * 234): the last message of the chat, of an agent that may be asked, while
   * nothing is being generated. The agent has the last word; this is an
   * offer, not a promise.
   */
  const mayRetry = (msg: Message) => {
    if (!takesPart || answers.length > 0 || !msg.id || !msg.sender_id || !mayRetryOf(actorKey(msg.sender_id))) return false;
    const last = [...messages].reverse().find(m => m.id && !m.revoked);
    return last?.id === msg.id && !msg.internal;
  };

  const retry = async (msg: Message) => {
    try {
      await api.retryMessage(sessionId, channelId, msg.id!);
    } catch (error) {
      reportError('Failed to ask for another answer', error);
    }
  };

  /**
   * Whether the user may ask for an answer anew after the turn given up that
   * the notice `msg` tells of (issue 259), one that may be answered again,
   * while it is the last thing in the chat - as `mayRetry` has it. The agent
   * has the last word here, too.
   */
  const mayRetryGivenUp = (msg: Message) =>
    !!msg.turn_id && !!msg.givenUp && mayBeAnsweredAgain(msg.givenUp) && mayRetry(msg);

  const retryGivenUp = async (msg: Message) => {
    const uuid = msg.sender_id ? participantUuid(actorKey(msg.sender_id)) : undefined;
    if (!uuid || !msg.turn_id) return;
    try {
      await api.retryAnswer(sessionId, channelId, uuid, msg.turn_id);
    } catch (error) {
      reportError('Failed to ask for another answer', error);
    }
  };

  /** The icon on the notice of a turn given up that asks for an answer anew, where it may. */
  const retryGivenUpAction = (msg: Message) => mayRetryGivenUp(msg) ? (
    <button
      onClick={() => void retryGivenUp(msg)}
      className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
      title="Ask for an answer anew"
      aria-label="Ask for an answer anew"
    >
      <RotateCcw size={13} />
    </button>
  ) : undefined;

  /** Takes `msg` back on the second click; the first asks for it. */
  const takeBack = async (msg: Message) => {
    if (revoking !== msg.id) {
      setRevoking(msg.id!);
      return;
    }
    setRevoking(null);
    try {
      await api.revokeMessage(sessionId, channelId, msg.id!);
    } catch (error) {
      reportError('Failed to take the message back', error);
    }
  };

  /** Deletes the agent `participant` (issues 164, 314): it leaves every channel, this one too, until it is restored. */
  const deleteAgent = async (participant: ChannelParticipant) => {
    const uuid = actorUuid(participant.id);
    if (!uuid) return;
    try {
      await api.deleteAgent(uuid);
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
    // Who it is is public (issue 244); an approver is nobody to introduce.
    ...(participant.kind !== 'approver' ? [{
      label: 'Show profile', icon: IdCard, title: `Show who ${participant.name} is`,
      onSelect: () => showOf(participant, 'profile'),
    }] : []),
    ...contextActions(participant),
    ...(participant.can_read_tools ? [{
      label: 'Show tools', icon: Wrench, title: `Show the tools of ${participant.name}`,
      onSelect: () => showOf(participant, 'tools'),
    }] : []),
    // The approver belongs to every channel and cannot be removed.
    ...(channel?.can_change && participant.kind !== 'approver' ? [{
      label: 'Remove from channel', icon: UserMinus, danger: true, title: `Remove ${participant.name} from the channel`,
      onSelect: () => void removeParticipant(participant),
    }] : []),
    ...(participant.can_delete ? [{
      label: 'Delete agent', icon: Trash2, danger: true, confirm: 'Delete agent? It leaves every channel until it is restored',
      title: `${participant.name} leaves every channel it takes part in, not only this one`,
      onSelect: () => void deleteAgent(participant),
    }] : []),
  ];

  /** Reading what `participant` heard, said and kept to itself - for a former participant too - where it may be read. */
  const contextActions = (participant: ChannelParticipant): MenuAction[] => participant.can_read_context ? [{
    label: 'Show history', icon: History, title: `Show what ${participant.name} heard and said`,
    onSelect: () => setHistoryOf({channelId, participant}),
  }, {
    label: 'Show private notes', icon: Lock, title: `Show what ${participant.name} noted for itself in this session`,
    onSelect: () => showOf(participant, 'privateNotes'),
  }] : [];

  const removeParticipant = async (participant: ChannelParticipant) => {
    const uuid = actorUuid(participant.id);
    if (!uuid) return;
    try {
      await api.removeParticipant(sessionId, channelId, uuid);
      setChannel(prev => prev && {...prev, participants: prev.participants.filter(m => actorKey(m.id) !== actorKey(participant.id))});
    } catch (error) {
      reportError(`Failed to remove ${participant.name}`, error);
    }
  };

  useEffect(() => {
    const handleSnapshot = (data: string) => {
      try {
        const snapshot: ChannelSnapshot = JSON.parse(data);
        setMessages(chatMessages(snapshot.items, nameIn(snapshot.channel)));
        setChannel(snapshot.channel);
        setUsage(Object.fromEntries(snapshot.channel.participants.flatMap(m => m.usage ? [[actorKey(m.id), m.usage]] : [])));
        countedCalls.current = new Set(snapshot.items.flatMap(item => 'said' in item && item.said.id ? [item.said.id] : []));
        setNotes(snapshot.notes ?? []);
        setPendingApprovals(snapshot.pending_approvals ?? []);
        setPendingQuestions(snapshot.pending_questions ?? []);
        setEarlier(snapshot.earlier);
        // The answers under way as the snapshot found them (issue 240): what
        // streamed before is not kept, what follows streams in.
        setAnswers(snapshot.channel.participants.flatMap(m => m.turn_under_way
          ? [{sender: actorKey(m.id), name: m.name, turnId: m.turn_under_way, reasoning: '', content: '', internal: false}]
          : []));
      } catch (e) {
        reportError('Failed to read the channel state', e);
      }
    };

    const handleMessage = (data: string) => {
      try {
        const envelope: Envelope = JSON.parse(data);
        const actorEvent = envelope.event;
        const answerer = actorKey(envelope.context.sender_id);
        const answererName = envelope.context.sender_name || null;
        // The answer of the envelope's sender, if it has one, has ended.
        const answerEnded = () => setAnswers(prev => prev.filter(a => a.sender !== answerer));
        // What the sender's calls used, as the server sums them once its
        // turn ended - also those whose messages were not published.
        const readUsage = () => {
          const agent = envelope.context.sender_id;
          const uuid = typeof agent === 'object' && 'Agent' in agent ? actorUuid(agent) : null;
          if (!uuid) return;
          api.getParticipant(sessionId, channelId, uuid)
            .then(participant => { if (participant.usage) setUsage(prev => ({...prev, [answerer]: participant.usage!})); })
            .catch(error => reportError('Failed to read what the agent used', error));
        };
        if (actorEvent === 'ResponseRequested') {
          // Someone asks you to respond (issue 278): shown, but not part of
          // the history - an agent is asked so, too, but a user only by a
          // nudge.
          const notice: Message = {
            role: 'System',
            content: null,
            sender_name: answererName ?? undefined,
            nudged: true,
            timestamp: envelope.timestamp,
          };
          setMessages(prev => [...prev, notice]);
        } else if (typeof actorEvent === 'string') {
          // The other payload-less events have nothing to show.
        } else if (actorEvent.ResponseStarted) {
          // A new answer begins after those under way, as the turn it names.
          const turnId = actorEvent.ResponseStarted.turn_id;
          setAnswers(prev => withAnswer(prev.filter(a => a.sender !== answerer), answerer, answererName, a => ({...a, turnId})));
        } else if (actorEvent.StateChanged) {
          // Where an agent stands now (issue 243), shown by its dot.
          const state = actorEvent.StateChanged;
          setChannel(prev => prev && {...prev, participants: prev.participants.map(m => actorKey(m.id) === answerer ? {...m, state} : m)});
        } else if (actorEvent.PartialMessageGenerated || actorEvent.IntermediatePartialGenerated) {
          // A step of a plan streams in as the answer does (issue 258).
          const partial = (actorEvent.PartialMessageGenerated ?? actorEvent.IntermediatePartialGenerated)!;
          const internal = !!actorEvent.IntermediatePartialGenerated;
          setAnswers(prev => withAnswer(prev, answerer, answererName, a => ({
            ...a,
            turnId: a.turnId ?? partial.turn_id ?? null,
            internal,
            reasoning: a.reasoning + (partial.reasoning ?? ''),
            content: a.content + (partial.content ?? ''),
          })));
        } else if (actorEvent.CompleteMessageGenerated || actorEvent.IntermediateMessageGenerated) {
          // A step of a plan is shown as any message, folded with the
          // other steps of its turn (issue 258).
          const newMsg = (actorEvent.CompleteMessageGenerated ?? actorEvent.IntermediateMessageGenerated)!;
          // A tool call that ended may have changed the files.
          if (newMsg.tool_results?.length) setWorkspaceVersion(v => v + 1);
          setMessages(prev => {
            // A message can arrive twice: in the snapshot and as an event.
            if (newMsg.id && prev.some(m => m.id === newMsg.id)) return prev;
            if (newMsg.tool_results && newMsg.tool_results.length > 0) {
              const lastMsg = prev[prev.length - 1];
              if (lastMsg && lastMsg.tool_calls && lastMsg.tool_calls.some(tc => newMsg.tool_results?.some(tr => tr.id === tc.id))) {
                const updatedLastMsg = {
                  ...lastMsg,
                  tool_results: [...(lastMsg.tool_results || []), ...newMsg.tool_results]
                };
                return [...prev.slice(0, -1), updatedLastMsg];
              }
            }
            return [...prev, newMsg];
          });
          // What its call used, at once: a plan's steps are many calls of
          // one turn, whose end would come too late for the gauge.
          if (newMsg.id && !countedCalls.current.has(newMsg.id)) {
            countedCalls.current.add(newMsg.id);
            setUsage(prev => {
              const used = withCall(prev[answerer], newMsg);
              return used ? {...prev, [answerer]: used} : prev;
            });
          }
          // Its sender may go on: what streams in next is its next message.
          const turnId = newMsg.turn_id ?? null;
          setAnswers(prev => prev.map(a => a.sender === answerer ? {...a, turnId: a.turnId ?? turnId, reasoning: '', content: ''} : a));
        } else if (actorEvent.MessageModified) {
          // In its place, as the server reads it (`Message::modify`, issue
          // 234): its words, classification and mentions, and when.
          const modified = actorEvent.MessageModified;
          setMessages(prev => prev.map(m => m.id === modified.id
            ? {...m, content: modified.content, reasoning: modified.reasoning, classification: modified.classification, mentions: modified.mentions, modified: modified.timestamp}
            : m));
        } else if (actorEvent.MessageRevoked) {
          // In its place without what it said (`Message::revoke`).
          const {message, at} = actorEvent.MessageRevoked;
          setMessages(prev => prev.map(m => m.id === message
            ? {...m, content: null, reasoning: undefined, tool_calls: [], tool_results: [], mentions: [], revoked: at}
            : m));
        } else if (actorEvent.MessageReclassified) {
          // In its place, classified anew (`Message::reclassify`, issue 239).
          const {message, classification} = actorEvent.MessageReclassified;
          setMessages(prev => prev.map(m => m.id === message ? {...m, classification} : m));
        } else if (actorEvent.RetryAccepted) {
          // A new answer begins: shown as under way from now on. An answer
          // under way was given up, and what of it was said stays, as the
          // history keeps it (issue 258); a completed answer is taken back
          // as `MessageRevoked` says.
          setAnswers(prev => withAnswer(prev.filter(a => a.sender !== answerer), answerer, answererName, a => a));
        } else if (actorEvent.RetryRejected) {
          // Shown, but not part of the history.
          const notice: Message = {
            role: 'System',
            content: null,
            sender_name: envelope.context.sender_name,
            retryRejected: actorEvent.RetryRejected.reason,
            timestamp: envelope.timestamp,
          };
          setMessages(prev => [...prev, notice]);
        } else if (actorEvent.MessageRejected) {
          // What the user sent, which the channel did not let in (issue
          // 304). A message of theirs, shown as they sent it, is marked as
          // not let in. Anything else is told in a notice, not part of the
          // history; a change of a message, shown as they made it, is
          // undone by what the channel holds.
          const {rejected, message, reason} = actorEvent.MessageRejected;
          if (message && rejected === 'CompleteMessageGenerated') {
            setMessages(prev => prev.map(m => m.id === message ? {...m, rejected: reason} : m));
          } else {
            if (message && ['MessageModified', 'MessageRevoked', 'MessageReclassified'].includes(rejected)) {
              api.getMessage(sessionId, channelId, message)
                .then(standing => setMessages(prev => prev.map(m => m.id === message ? standing : m)))
                .catch(error => reportError('Failed to read the message back', error));
            }
            const notice: Message = {
              role: 'System',
              content: null,
              messageRejected: {rejected, reason},
              timestamp: envelope.timestamp,
            };
            setMessages(prev => [...prev, notice]);
          }
        } else if (actorEvent.ApprovalRequested) {
          // Asked again by an agent that resumed its turn (issue 172), or
          // already in the snapshot: shown once.
          const requested = actorEvent.ApprovalRequested;
          setPendingApprovals(prev => prev.some(req => req.id === requested.id) ? prev : [...prev, requested]);
        } else if (actorEvent.ApprovalGranted || actorEvent.ApprovalDenied || actorEvent.ApprovalTimedOut || actorEvent.ApprovalRequestWithdrawn) {
          // Over only once the approver says so - decided, or no longer
          // asked for (issue 148); a user's answer alone may not decide it.
          const sender = envelope.context.sender_id;
          const ended = (actorEvent.ApprovalGranted ?? actorEvent.ApprovalDenied ?? actorEvent.ApprovalTimedOut ?? actorEvent.ApprovalRequestWithdrawn)!.id;
          // This user's own answer, as their actor shows it: given by a rule
          // of theirs (issue 13).
          const byRule = (actorEvent.ApprovalGranted ?? actorEvent.ApprovalDenied)?.by_rule;
          if (byRule && typeof sender === 'object' && 'User' in sender && sender.User === user.id) {
            setAnsweredApprovals(prev => prev.includes(ended) ? prev : [...prev, ended]);
            setAnsweredByRule(prev => ({...prev, [ended]: byRule}));
          }
          if (typeof sender === 'object' && 'Approver' in sender) {
            setPendingApprovals(prev => prev.filter(req => req.id !== ended));
            setAnsweredApprovals(prev => prev.filter(id => id !== ended));
          }
        } else if (actorEvent.QuestionRequested) {
          // Asked again by an agent that resumed its turn, or already in the
          // snapshot: shown once.
          const requested = actorEvent.QuestionRequested;
          setPendingQuestions(prev => prev.some(q => q.id === requested.id) ? prev : [...prev, requested]);
        } else if (actorEvent.QuestionClosed) {
          const closed = actorEvent.QuestionClosed.id;
          setPendingQuestions(prev => prev.filter(q => q.id !== closed));
          setAnsweredQuestions(prev => prev.filter(id => id !== closed));
        } else if (actorEvent.TurnFinished) {
          // Its messages were shown as they were said.
          const underWay = activityUnderWay(actorEvent.TurnFinished.turn_id);
          setMessages(prev => prev.filter(m => m.id !== underWay));
          answerEnded();
          readUsage();
        } else if (actorEvent.TurnGivenUp) {
          // Shown in the chat as the turn's end (issue 258), not yet part of
          // the history: gone on reload.
          const {turn_id, reason} = actorEvent.TurnGivenUp;
          const notice = givenUpNotice(turn_id, reason, envelope.context.sender_id, envelope.context.sender_name, envelope.timestamp, envelope.context.span_id);
          // What it did of its own accord ended with it (issue 284).
          const underWay = activityUnderWay(turn_id);
          setMessages(prev => prev.some(m => m.id === notice.id) ? prev.filter(m => m.id !== underWay) : [...prev.filter(m => m.id !== underWay), notice]);
          answerEnded();
          readUsage();
        } else if (actorEvent.CommandExecuted) {
          const {command, args, output, to, turn_id} = actorEvent.CommandExecuted;
          if (turn_id) {
            // Carried out by an agent (issue 269): part of the history,
            // kept once by its turn - live with what it answered, and to
            // whom: the stream carries `to` only to the issuer.
            const notice = commandNotice(turn_id, command, args, envelope.context.sender_id, envelope.context.sender_name, undefined, envelope.timestamp, {output, private: to !== null});
            setMessages(prev => prev.some(m => m.id === notice.id) ? prev.map(m => m.id === notice.id ? notice : m) : [...prev, notice]);
            // A command turn ends without `TurnFinished`, and may have
            // compacted the context (issue 405): what the agent uses is
            // read again.
            readUsage();
          } else {
            // A command of the channel: shown, but not part of the history.
            const notice: Message = {
              role: 'System',
              content: output,
              sender_name: envelope.context.sender_name,
              commandOutput: {command, private: false},
              timestamp: envelope.timestamp,
            };
            setMessages(prev => [...prev, notice]);
          }
        } else if (actorEvent.ActivityStarted) {
          // An agent does something of its own accord (issue 284): its
          // notice shows it under way until it ends, and its dot says what.
          const {turn_id, activity} = actorEvent.ActivityStarted;
          const notice = activityNotice(turn_id, activity, envelope.context.sender_id, envelope.context.sender_name, envelope.timestamp);
          setMessages(prev => prev.some(m => m.id === notice.id) ? prev : [...prev, notice]);
          setActivities(prev => ({...prev, [answerer]: activity}));
        } else if (actorEvent.ActivityFinished) {
          // In the place of its start, with what came of it: part of the
          // history, kept once by when it ended. The context may have
          // shrunk: what the agent uses is read again.
          const {turn_id, activity, output} = actorEvent.ActivityFinished;
          const notice = activityNotice(turn_id, activity, envelope.context.sender_id, envelope.context.sender_name, envelope.timestamp, output);
          const underWay = activityUnderWay(turn_id);
          setMessages(prev => prev.some(m => m.id === notice.id)
            ? prev.filter(m => m.id !== underWay)
            : prev.some(m => m.id === underWay) ? prev.map(m => m.id === underWay ? notice : m) : [...prev, notice]);
          readUsage();
        } else if (actorEvent.CommandStarted) {
          // An agent began a command of yours (issue 299): its notice shows
          // it under way until the answer, kept by the same turn, replaces it.
          const {command, args, turn_id} = actorEvent.CommandStarted;
          const begun = commandNotice(turn_id, command, args, envelope.context.sender_id, envelope.context.sender_name, undefined, envelope.timestamp);
          const notice = {...begun, commandOutput: {...begun.commandOutput!, underWay: true}};
          setMessages(prev => prev.some(m => m.id === notice.id) ? prev : [...prev, notice]);
        } else if (actorEvent.CommandFailed) {
          // An agent did not carry out a command (issue 269): for the issuer
          // alone, and gone on reload - nothing happened. In the place of its
          // start, if it began.
          const {command, reason, turn_id} = actorEvent.CommandFailed;
          const notice: Message = {
            id: turn_id ? `command:${turn_id}` : undefined,
            role: 'System',
            content: reason,
            sender_name: envelope.context.sender_name,
            commandOutput: {command, private: true, agent: envelope.context.sender_name, failed: true},
            timestamp: envelope.timestamp,
          };
          setMessages(prev => notice.id && prev.some(m => m.id === notice.id) ? prev.map(m => m.id === notice.id ? notice : m) : [...prev, notice]);
        } else if (actorEvent.ParticipantJoined) {
          // Part of the history (issue 408), kept once by whom and when.
          const {participant, name} = actorEvent.ParticipantJoined;
          const notice = participantNotice('joined', participant, name, envelope.context.sender_id, envelope.context.sender_name, envelope.timestamp);
          setMessages(prev => prev.some(m => m.id === notice.id) ? prev : [...prev, notice]);
        } else if (actorEvent.ParticipantLeft) {
          // Nothing of the participant that left follows (issue 162): what
          // it had under way has ended - its answer, its calls of tools.
          // That it left is part of the history (issue 408).
          const {participant, name} = actorEvent.ParticipantLeft;
          const left = actorKey(participant);
          const notice = participantNotice('left', participant, name, envelope.context.sender_id, envelope.context.sender_name, envelope.timestamp);
          setMessages(prev => {
            const ended = prev.map(m =>
              m.sender_id && actorKey(m.sender_id) === left && m.tool_calls?.some(c => !m.tool_results?.some(r => r.id === c.id))
                ? {...m, abandoned: true}
                : m
            );
            return ended.some(m => m.id === notice.id) ? ended : [...ended, notice];
          });
          setAnswers(prev => prev.filter(a => a.sender !== left));
        } else if (actorEvent.ChainCutOff) {
          // Like a failed turn: shown, but not part of the history.
          const notice: Message = {
            role: 'System',
            content: null,
            chainCutOff: {maxAgentHops: actorEvent.ChainCutOff.max_agent_hops, notAsked: actorEvent.ChainCutOff.not_asked},
            timestamp: envelope.timestamp,
          };
          setMessages(prev => [...prev, notice]);
        } else if (actorEvent.PlanStepStarted) {
          // Heads the messages of the step that follow (issue 263), as the
          // history's mark of it does after a reload.
          const {turn_id, ...step} = actorEvent.PlanStepStarted;
          const mark = planStepMark(turn_id, step, envelope.context.sender_id, envelope.context.sender_name, envelope.timestamp);
          setMessages(prev => prev.some(m => m.id === mark.id) ? prev : [...prev, mark]);
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
  }, [sessionId, channelId, takesPart, user.id]);


  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, answers]);

  useEffect(() => {
    saveDraft(channelId, inputText);
  }, [channelId, inputText]);

  // The newest message of someone else shown is what the user read (issue
  // 404), marked while the tab is visible: as the channel is opened, as
  // messages arrive and as the tab comes back. Never one of their own, which
  // is shown before the channel takes it in; nor a notice of the UI's, which
  // is in no history.
  const newestOfOthers = useMemo(() => messages.findLast(m =>
    m.id && (m.role === 'User' || m.role === 'Assistant') && isOthers(m, me)
    && !m.givenUp && !m.commandOutput && !m.activity && !m.chainCutOff && !m.nudged
  )?.id ?? null, [messages, me]);
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
    if (earlier === null) return;
    setIsLoadingHistory(true);
    try {
      const page = await api.getHistory(sessionId, channelId, earlier);
      const data = chatMessages(page.items, nameIn(channel));
      setMessages(prev => {
        const known = new Set(data.map(m => m.id));
        return [...data, ...prev.filter(m => !m.id || !known.has(m.id))];
      });
      setEarlier(page.earlier);
    } catch (error) {
      reportError('Failed to load earlier messages', error);
    } finally {
      setIsLoadingHistory(false);
    }
  };

  const handleSendMessage = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputText.trim() || isSending) return;

    const text = inputText;
    setInputText('');
    setIsSending(true);

    // Optimistic update
    //const userMsg: Message = { role: 'User', content: text };
    //setMessages(prev => [...prev, userMsg]);

    const input = parseInput(text);
    try {
      if (input.kind === 'command') {
        const reply = await api.executeCommand(sessionId, channelId, input.name, input.args);
        // An answer for the whole channel arrives on the stream instead.
        if (reply.output !== null) {
          const notice: Message = {
            role: 'System',
            content: reply.output,
            commandOutput: {command: reply.command, private: true},
            // Private output has no envelope: the time it arrived.
            timestamp: new Date().toISOString(),
          };
          setMessages(prev => [...prev, notice]);
        }
      } else {
        await api.sendMessage(sessionId, channelId, input.text, release ?? undefined);
        // A release is for one message.
        setRelease(null);
      }
    } catch (error) {
      // The server's detail for a command already names it.
      reportError(input.kind === 'command' ? 'Command failed' : 'Failed to send message', error);
      // Give the text back rather than losing it.
      setInputText(prev => prev || text);
    } finally {
      setIsSending(false);
    }
  };

  const updateInput = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInputText(e.target.value);
    setCaret(e.target.selectionStart ?? e.target.value.length);
    setSuggestionIndex(0);
  };

  const selectCommand = (command: Command, handle?: string) => {
    // The rest of the name typed, and the blanks after it - not a line break.
    const rest = inputText.slice(caret).replace(/^\S*[^\S\n]*/, '');
    const inserted = handle ? `/${command.name} @${handle} ` : `/${command.name} `;
    const newCaret = inserted.length;
    setInputText(inserted + rest);
    setCaret(newCaret);
    setSuggestionIndex(0);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(newCaret, newCaret);
    });
  };

  const selectSuggestion = (suggestion: Suggestion) => {
    if (suggestion.kind === 'command') selectCommand(suggestion.command, suggestion.handle);
    else selectMention(suggestion.kind === 'group' ? suggestion.handle : suggestion.participant.handle);
  };

  const selectMention = (handle: string) => {
    if (!mentionQuery) return;
    const before = inputText.slice(0, mentionQuery.start);
    const after = inputText.slice(caret).replace(/^\s*/, ' ');
    const inserted = `@${handle}`;
    const newCaret = before.length + inserted.length + 1;
    setInputText(before + inserted + after);
    setCaret(newCaret);
    setSuggestionIndex(0);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(newCaret, newCaret);
    });
  };

  const handleInputKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // A key that ends an input method's composition is the composition's.
    if (e.nativeEvent.isComposing) return;
    // Shift+Enter is a line break, also while completing (issue 274).
    const enter = e.key === 'Enter' && !e.shiftKey;
    if (suggestions.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSuggestionIndex(i => (i + 1) % suggestions.length);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSuggestionIndex(i => (i - 1 + suggestions.length) % suggestions.length);
      } else if (enter || e.key === 'Tab') {
        e.preventDefault();
        selectSuggestion(suggestions[Math.min(suggestionIndex, suggestions.length - 1)]);
      } else if (e.key === 'Escape') {
        e.preventDefault();
        setDismissedMention(suggestionStart);
      }
    } else if (enter) {
      e.preventDefault();
      e.currentTarget.form?.requestSubmit();
    }
  };

  /** What a chain cut off at its limit says (issue 200), naming the agents mentioned as they are known now. */
  const chainCutOffText = ({maxAgentHops, notAsked}: {maxAgentHops: number; notAsked: ActorId[]}): string => {
    const names = notAsked.map(id => {
      const participant = participants.find(m => actorKey(m.id) === actorKey(id));
      return participant?.handle ? `@${participant.handle}` : (participant?.name ?? 'an agent');
    });
    const whom = names.length > 0 ? `${names.join(', ')} ${names.length === 1 ? 'was' : 'were'} not asked` : 'no agent was asked';
    return `The chain between agents reached its limit of ${maxAgentHops} ${maxAgentHops === 1 ? 'hop' : 'hops'}, so ${whom} to respond. A message from a user starts a new chain.`;
  };

  /** Whether the user may see the context of `msg`: a stored message of an agent they may read (issue 41). */
  const mayShowContext = (msg: Message): boolean => {
    if (!msg.id || !msg.sender_id) return false;
    const sender = actorKey(msg.sender_id);
    return participants.some(m => actorKey(m.id) === sender && m.can_read_context);
  };

  const handleStopGeneration = async () => {
    try {
      await api.cancelChannel(sessionId, channelId);
    } catch (error) {
      reportError('Failed to cancel generation', error);
    }
  };

  /** The id the REST API addresses the participant `sender` (an `actorKey`) by. */
  const participantUuid = (sender: string) => actorUuid(participants.find(m => actorKey(m.id) === sender)?.id);

  /** Whether the user may end the answer of `sender`: a participant whose `can_cancel` allows it (issues 235, 243). */
  const mayCancelOf = (sender: string) => participants.some(m => actorKey(m.id) === sender && m.can_cancel);

  /** Stops the answer of the agent `sender` alone (issue 235); the others go on. */
  const stopAnswer = async (sender: string) => {
    const uuid = participantUuid(sender);
    if (!uuid) return;
    try {
      await api.cancelParticipant(sessionId, channelId, uuid);
    } catch (error) {
      reportError('Failed to stop the answer', error);
    }
  };

  /**
   * Whether the user may interrupt a call of `msg` alone (issue 276): one
   * of its turn, while its agent runs tools and their `can_cancel` allows it.
   * Only calls without a result show the offer.
   */
  const mayInterruptCallsOf = (msg: Message) => {
    if (!msg.turn_id || !msg.sender_id) return false;
    const sender = actorKey(msg.sender_id);
    return participants.some(m => actorKey(m.id) === sender && m.can_cancel && m.state === 'ExecutingTools');
  };

  /** Interrupts the call `call` of `msg` alone; the agent's turn goes on (issue 276). */
  const interruptCall = async (msg: Message, call: ToolCall) => {
    const uuid = msg.sender_id ? participantUuid(actorKey(msg.sender_id)) : undefined;
    if (!uuid || !msg.turn_id || !call.id) return;
    try {
      await api.interruptToolCall(sessionId, channelId, uuid, msg.turn_id, call.id);
    } catch (error) {
      reportError('Failed to interrupt the tool call', error);
    }
  };

  // The commands ended in the channel: by a result that shows them so, or by the user (issue 294).
  const ended = useMemo(() => {
    const ids = endedCommands(messages);
    if (killedCommands?.channelId === channelId) killedCommands.ids.forEach(id => ids.add(id));
    return ids;
  }, [messages, killedCommands, channelId]);

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

  /**
   * Asks for another answer instead of `answer`, still under way (issue
   * 235): offered once its turn is known and where its answerer's
   * `can_retry` allows it (issue 242). Refused if the answer ended
   * meanwhile; the retry icon on its last message is then the way.
   */
  const retryAnswer = async (answer: Answer) => {
    const uuid = participantUuid(answer.sender);
    if (!uuid || !answer.turnId) return;
    try {
      await api.retryAnswer(sessionId, channelId, uuid, answer.turnId);
    } catch (error) {
      reportError('Failed to ask for another answer', error);
    }
  };

  // Only those questions are put to answer them (issue 165); everybody else
  // they reach sees what the agent waits for.
  const askedOf = (request: QuestionRequest) =>
    request.questions.ask.flatMap(a => typeof a === 'object' && 'actor' in a ? [actorKey(a.actor)] : []);
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
    participants.some(m => actorKey(m.id) === actorKey({Agent: approval.agent_id}) && m.can_approve);

  // The answer first - the person decides this call now - then the rule
  // they chose to derive from it (issue 13); if the rule fails, the answer
  // stands.
  const handleApproval = async (approval: ApprovalRequest, approved: boolean) => {
    try {
      await api.respondToApproval(sessionId, channelId, approval.id, approval.agent_id, approved);
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

  // The heading of a command's notice: who ran it - or, for one of an agent
  // (issue 269), which agent carried it out or did not - and whether only
  // you see it.
  const commandHeading = (output: NonNullable<Message['commandOutput']>, sender: string | undefined) => {
    const name = <span className="font-mono normal-case">/{output.command}{output.args ? ` ${output.args}` : ''}</span>;
    const yours = output.private ? ' · only visible to you' : '';
    if (output.failed) return <span>{output.agent} did not carry out {name}{yours}</span>;
    if (output.underWay) {
      return (
        <span className="inline-flex items-center gap-1.5">
          {output.agent} is carrying out {name}
          <span className="w-1.5 h-1.5 bg-orange-500 rounded-full animate-command" />
        </span>
      );
    }
    if (output.agent) return <span>{output.agent} carried out {name}{output.issuer ? ` for ${output.issuer}` : ''}{yours}</span>;
    return output.private
      ? <span>{name} · only visible to you</span>
      : <span>{sender} ran {name}</span>;
  };

  // One message of the chat, as its kind shows it; `key` for the list it
  // is rendered in, `inTurn` where it stands among the messages of its turn
  // shown together (issue 415).
  const renderMessage = (msg: Message, key: React.Key, inTurn?: {first: boolean; last: boolean}) => msg.participant ? (
    <div
      key={key}
      className="flex items-center justify-center gap-2 text-xs text-gray-500 dark:text-gray-400"
      title={formatTimestamp(msg.timestamp)?.full}
    >
      {msg.participant === 'joined' ? <UserPlus size={14} /> : <UserMinus size={14} />}
      <span>{msg.content}</span>
    </div>
  ) : msg.activity ? (
    <div
      key={key}
      className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
    >
      <div className={cn("flex items-center gap-2 text-xs font-semibold uppercase tracking-wider opacity-70", msg.content && "mb-1")}>
        <Cog size={14} />
        <span className="inline-flex items-center gap-1.5">
          {msg.activity.agent} {msg.activity.underWay ? 'is' : 'was'} {msg.activity.activity}
          {msg.activity.underWay && <span className="w-1.5 h-1.5 bg-green-500 rounded-full animate-working" />}
        </span>
      </div>
      {msg.content && <MarkdownMessage content={msg.content} />}
      <MessageFooter message={msg} />
    </div>
  ) : msg.commandOutput ? (
    <div
      key={key}
      className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
        <ChevronRight size={14} />
        {commandHeading(msg.commandOutput, msg.sender_name)}
      </div>
      {msg.content && <MarkdownMessage content={msg.content} />}
      <MessageFooter message={msg} />
    </div>
  ) : msg.nudged ? (
    <div
      key={key}
      className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider opacity-70">
        <BellRing size={14} />
        <span>{msg.sender_name ?? 'Someone'} asks you to respond · only visible to you</span>
      </div>
      <MessageFooter message={msg} />
    </div>
  ) : msg.chainCutOff ? (
    <div
      key={key}
      className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
        <Unlink size={14} />
        <span>Chain cut off</span>
      </div>
      <div className="wrap-break-word">{chainCutOffText(msg.chainCutOff)}</div>
      <MessageFooter message={msg} />
    </div>
  ) : msg.givenUp?.kind === 'failed' ? (
    <div
      key={key}
      role="alert"
      className="max-w-[80%] p-3 rounded-lg border border-red-300 dark:border-red-800 bg-red-50 dark:bg-red-900/30 text-red-800 dark:text-red-200 text-sm"
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1">
        <AlertTriangle size={14} />
        {msg.sender_name ? `${msg.sender_name} could not answer` : 'The turn failed'}
      </div>
      <div className="wrap-break-word">{msg.content}</div>
      {msg.span_id && (
        <div className="mt-1 text-[11px] font-mono opacity-60">span_id {msg.span_id}</div>
      )}
      <MessageFooter message={msg} actions={retryGivenUpAction(msg)} />
    </div>
  ) : msg.givenUp ? (
    <div
      key={key}
      className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
        <Ban size={14} />
        <span>{msg.sender_name || 'The agent'} gave its answer up</span>
      </div>
      <div className="wrap-break-word">{msg.content}</div>
      <MessageFooter message={msg} actions={retryGivenUpAction(msg)} />
    </div>
  ) : msg.messageRejected ? (
    <div
      key={key}
      className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
        <Ban size={14} />
        <span>Not let in</span>
      </div>
      <div className="wrap-break-word">{rejectedText(msg.messageRejected.rejected)} was not let into the channel: {rejectionText(msg.messageRejected.reason)}.</div>
      <MessageFooter message={msg} />
    </div>
  ) : msg.retryRejected ? (
    <div
      key={key}
      className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
        <RotateCcw size={14} />
        <span>Not answered again</span>
      </div>
      <div className="wrap-break-word">{msg.sender_name || 'The agent'} did not answer again: {retryRefusedText(msg.retryRejected)}.</div>
      <MessageFooter message={msg} />
    </div>
  ) : msg.rejected ? (
    <div
      key={key}
      className={cn(
        "max-w-[80%] p-3 rounded-lg border border-dashed border-red-300 dark:border-red-800 text-gray-600 dark:text-gray-300 text-sm",
        isOwn(msg, me) && "ml-auto",
      )}
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
        <Ban size={14} />
        <span>Not let into the channel</span>
      </div>
      <div className="wrap-break-word whitespace-pre-wrap">{msg.content}</div>
      <div className="mt-1 text-[11px] opacity-70">{rejectionText(msg.rejected)}.</div>
    </div>
  ) : msg.revoked ? (
    <div
      key={key}
      className={cn(
        "max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 text-gray-500 dark:text-gray-400 text-sm italic",
        isOwn(msg, me) && "ml-auto",
      )}
    >
      {msg.sender_name || 'Its author'} took this message back
      <div className="mt-1 text-[11px] not-italic opacity-70" title={formatTimestamp(msg.revoked)?.full}>
        {formatTimestamp(msg.revoked)?.short}
      </div>
    </div>
  ) : editing === msg.id ? (
    <MessageEditor
      key={key}
      content={msg.content ?? ''}
      onSave={(content) => saveEdit(msg, content)}
      onCancel={() => setEditing(null)}
    />
  ) : (
    <MessageView
      key={key}
      message={msg}
      me={me}
      participants={participants}
      files={workspaceFiles ?? undefined}
      filesVersion={workspaceVersion}
      artifacts={channelArtifacts}
      onSelectPage={onSelectPage}
      onInterruptCall={mayInterruptCallsOf(msg) ? (call) => void interruptCall(msg, call) : undefined}
      onKillCommand={workspace?.can_write ? (commandId) => void killCommand(commandId) : undefined}
      endedCommands={ended}
      inTurn={inTurn}
      badges={channel && msg.classification && !sameClassification(msg.classification, channel.classification)
        ? [`released: ${classificationLabel(msg.classification)}`]
        : undefined}
      actions={(mayShowContext(msg) || mayReclassify(msg) || mayChange(msg) || mayRetry(msg)) ? (
        <>
          {mayRetry(msg) && (
            <button
              onClick={() => void retry(msg)}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Ask for another answer instead of this one"
              aria-label="Ask for another answer instead of this one"
            >
              <RotateCcw size={13} />
            </button>
          )}
          {mayChange(msg) && (
            <button
              onClick={() => { setRevoking(null); setEditing(msg.id!); }}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Edit the message"
              aria-label="Edit the message"
            >
              <Pencil size={13} />
            </button>
          )}
          {mayChange(msg) && (
            <button
              onClick={() => void takeBack(msg)}
              onBlur={() => setRevoking(prev => prev === msg.id ? null : prev)}
              className={cn("p-0.5 flex items-center gap-1", revoking === msg.id ? "opacity-100 font-semibold" : "opacity-60 hover:opacity-100 focus:opacity-100")}
              title="Take the message back - it stays in the record of who saw it"
              aria-label={revoking === msg.id ? "Take the message back? Click again" : "Take the message back"}
            >
              <Undo2 size={13} />
              {revoking === msg.id && <span>Take back?</span>}
            </button>
          )}
          {mayReclassify(msg) && (
            <button
              onClick={() => setReclassifying({kind: 'message', message: msg})}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Change the classification"
              aria-label="Change the classification"
            >
              <Shield size={13} />
            </button>
          )}
          {mayShowContext(msg) && (
            <button
              onClick={() => setContextOf({channelId, message: msg})}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Show what the agent saw"
              aria-label="Show what the agent saw"
            >
              <ScrollText size={13} />
            </button>
          )}
        </>
      ) : undefined}
    />
  );

  const rows = chatRows(mergeToolResults(messages));
  // The row of each turn's step that came last: the step it is in now, open
  // while the turn is under way (issue 263).
  const currentSteps = new Map(rows.flatMap((row, i) => row.kind === 'steps' ? [[row.turnId, i] as const] : []));
  // An answer that streams in a step of a plan is shown in the step's block;
  // where its turn has none - a snapshot without the step's mark - below,
  // with every other answer.
  const inStep = (answer: Answer) => answer.internal && currentSteps.has(answer.turnId);
  // The row before which the new messages begin (issue 404), if there were
  // any as the channel was opened: after the one last read, or at the top
  // where it lies before what is shown or nothing was read.
  const newFrom = (() => {
    if (newSince?.channelId !== channelId || newSince.count === 0 || rows.length === 0) return null;
    const read = newSince.after === null ? -1 : rows.findIndex(row => row.kind === 'message'
      ? row.message.id === newSince.after
      : row.messages.some(m => m.id === newSince.after));
    const from = read + 1;
    return from < rows.length ? from : null;
  })();
  // The rows grouped by turn (issue 415), and how many rows each turn has in
  // all - also where answers interleaved split it -, with its answer under
  // way: the messages of a turn of more than one are shown together when
  // one of them is pointed at.
  const groups = turnGroups(rows, newFrom);
  const turnSizes = new Map<string, number>();
  for (const row of rows) {
    const turnId = rowTurn(row);
    if (turnId !== null) turnSizes.set(turnId, (turnSizes.get(turnId) ?? 0) + 1);
  }
  // An answer under way goes on with the last group where that is of its
  // turn; any other is shown below every group, with its name.
  const lastTurn = groups.length > 0 ? groups[groups.length - 1].turnId : null;
  const continuesLast = (answer: Answer) => !inStep(answer) && answer.turnId !== null && answer.turnId === lastTurn;
  if (lastTurn !== null) {
    const continuing = answers.filter(continuesLast).length;
    turnSizes.set(lastTurn, (turnSizes.get(lastTurn) ?? 0) + continuing);
  }
  // A row's key, stable while earlier pages are put in front of it.
  const rowKey = (row: ChatRow, idx: number) => row.kind === 'message'
    ? row.message.id ?? `local-${idx}`
    : `steps-${row.turnId}-${row.step?.step_index ?? ''}-${row.messages[0]?.id ?? idx}`;

  // An answer under way; `continues` where it follows the messages of its
  // turn shown together, which name the agent already (issue 415).
  const renderAnswer = (answer: Answer, continues = false) => (
    <div key={answer.sender} className="max-w-[80%] p-3 rounded-lg shadow-xs bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600">
      <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider flex items-center gap-2">
        {!continues && <span>{answer.name || 'Assistant'}</span>}
        <span className="inline-flex items-center gap-1.5 normal-case font-normal text-blue-500 dark:text-blue-400">
          <span className="w-1.5 h-1.5 bg-green-500 rounded-full animate-working" />
          generating…
        </span>
        <span className="ml-auto inline-flex items-center gap-1">
          {answer.turnId && mayRetryOf(answer.sender) && (
            <button
              type="button"
              onClick={() => void retryAnswer(answer)}
              className="p-0.5 rounded-sm text-gray-400 hover:text-blue-600 dark:hover:text-blue-400 transition-colors"
              title="Ask for another answer instead of this one"
              aria-label="Ask for another answer instead of this one"
            >
              <RotateCcw size={12} />
            </button>
          )}
          {mayCancelOf(answer.sender) && (
            <button
              type="button"
              onClick={() => void stopAnswer(answer.sender)}
              className="p-0.5 rounded-sm text-gray-400 hover:text-red-600 dark:hover:text-red-400 transition-colors"
              title="Stop this answer alone"
              aria-label="Stop this answer alone"
            >
              <Square size={12} fill="currentColor" />
            </button>
          )}
        </span>
      </div>
      {answer.reasoning && <Reasoning text={answer.reasoning} thinking={!answer.content} />}
      {answer.content && <MarkdownMessage content={answer.content} />}
    </div>
  );

  return (
    <div className="flex-1 flex overflow-hidden bg-gray-50 dark:bg-gray-800">
      <div className="flex-1 flex flex-col min-w-0">
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {earlier !== null && (
            <div className="text-center">
              <button
                onClick={loadEarlier}
                disabled={isLoadingHistory}
                className="text-xs px-3 py-1 rounded-md border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800 disabled:opacity-50"
              >
                {isLoadingHistory ? 'Loading…' : 'Load earlier messages'}
              </button>
            </div>
          )}
          {groups.map(group => {
            const trailing = group === groups[groups.length - 1] ? answers.filter(continuesLast) : [];
            const together = group.turnId !== null && (turnSizes.get(group.turnId) ?? 0) > 1;
            return [group.start === newFrom && (
              <div key="new-messages" role="separator" aria-label="New messages" className="flex items-center gap-2 text-xs font-semibold text-red-600 dark:text-red-400">
                <div className="flex-1 border-t border-red-400 dark:border-red-500" />
                New messages
                <div className="flex-1 border-t border-red-400 dark:border-red-500" />
              </div>
            ), (
              <div
                key={`turn-${rowKey(rows[group.start], group.start)}`}
                onMouseEnter={together ? () => setPointedTurn(group.turnId) : undefined}
                onMouseLeave={together ? () => setPointedTurn(null) : undefined}
                className={cn(
                  "space-y-1",
                  // Indented as a whole, its frame flush with the messages alone and as
                  // far from its messages as they are from their content.
                  together && "p-3 rounded-lg transition-colors",
                  together && pointedTurn === group.turnId && "bg-gray-200/70 dark:bg-gray-700/50 ring-1 ring-gray-300 dark:ring-gray-600",
                )}
              >
                {rows.slice(group.start, group.end).map((row, offset) => {
                  const idx = group.start + offset;
                  const inTurn = {first: idx === group.start, last: idx === group.end - 1 && trailing.length === 0};
                  if (row.kind === 'message') return renderMessage(row.message, rowKey(row, idx), inTurn);
                  const current = currentSteps.get(row.turnId) === idx;
                  const streaming = current ? answers.filter(a => a.turnId === row.turnId && inStep(a)) : [];
                  return (
                    <PlanSteps
                      key={rowKey(row, idx)}
                      name={row.sender || 'Assistant'}
                      step={row.step}
                      count={row.messages.length}
                      underWay={row.turnId !== null && current && answers.some(a => a.turnId === row.turnId)}
                      streaming={streaming.length > 0}
                    >
                      {row.messages.map((msg, i) => renderMessage(msg, msg.id ?? `local-${i}`, {first: false, last: i === row.messages.length - 1 && streaming.length === 0}))}
                      {streaming.map(answer => renderAnswer(answer, true))}
                    </PlanSteps>
                  );
                })}
                {trailing.map(answer => renderAnswer(answer, true))}
              </div>
            )];
          })}
          {answers.filter(answer => !inStep(answer) && !continuesLast(answer)).map(answer => renderAnswer(answer))}
          <div ref={messagesEndRef} />
        </div>

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
                asked={askedOf(request).includes(actorKey(me))}
                askedNames={askedOf(request).map(key => participants.find(m => actorKey(m.id) === key)?.name ?? key)}
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

        <div className="p-4 bg-white dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700">
          {takesPart === false ? (
            <p className="text-sm text-gray-500 dark:text-gray-400 text-center p-2">
              You do not take part in this channel: its history is shown as it is, not followed live, and you cannot write here.
            </p>
          ) : (
          <form
            onSubmit={handleSendMessage}
            // The controls beside the field stay one line of it high, at its
            // bottom, while it grows (issue 274).
            className="flex items-end space-x-2"
          >
            {channel && (
              <div className="h-10.5 flex">
                <ClassificationPicker
                  channel={channel.classification}
                  compartments={compartments}
                  value={release ?? channel.classification}
                  onChange={(c) => setRelease(sameClassification(c, channel.classification) ? null : c)}
                />
              </div>
            )}
            <div className="relative flex-1 flex">
              {suggestions.length > 0 && (
                <ul
                  role="listbox"
                  className="absolute bottom-full left-0 mb-1 w-64 max-h-60 overflow-y-auto z-10 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded-md shadow-lg py-1"
                >
                  {suggestions.map((suggestion, idx) => {
                    const [label, detail, description] = suggestion.kind === 'command'
                      ? [`/${suggestion.command.name}`, [suggestion.handle && `@${suggestion.handle}`, suggestion.command.usage].filter(Boolean).join(' '), suggestion.command.description]
                      : suggestion.kind === 'group'
                        ? ['Everyone', `@${suggestion.handle}`, 'Every participant of this channel']
                        : [suggestion.participant.name, `@${suggestion.participant.handle}`, null];
                    return (
                      <li
                        key={`${suggestion.kind}:${label}:${suggestion.kind !== 'mention' ? suggestion.handle ?? '' : ''}`}
                        role="option"
                        aria-selected={idx === suggestionIndex}
                        onMouseDown={(e) => { e.preventDefault(); selectSuggestion(suggestion); }}
                        onMouseEnter={() => setSuggestionIndex(idx)}
                        className={cn(
                          "px-3 py-1.5 cursor-pointer",
                          idx === suggestionIndex
                            ? "bg-blue-600 text-white"
                            : "text-gray-800 dark:text-gray-200"
                        )}
                      >
                        <div className="flex items-baseline justify-between gap-2">
                          <span className={cn("text-sm truncate", suggestion.kind === 'command' && "font-mono")}>{label}</span>
                          <span className={cn("text-xs font-mono", idx === suggestionIndex ? "text-blue-100" : "text-gray-500 dark:text-gray-400")}>
                            {detail}
                          </span>
                        </div>
                        {description && (
                          <div className={cn("text-xs truncate", idx === suggestionIndex ? "text-blue-100" : "text-gray-500 dark:text-gray-400")}>
                            {description}
                          </div>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
              <GrowingTextarea
                ref={inputRef}
                value={inputText}
                onChange={updateInput}
                onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? inputText.length)}
                onKeyDown={handleInputKeyDown}
                onBlur={() => setDismissedMention(suggestionStart)}
                onFocus={() => setDismissedMention(null)}
                placeholder={answersOnlyWhenMentioned(channel?.responder_policy)
                  ? "Agents answer only when @-mentioned... (/ for commands)"
                  : "Type your message... (@ to mention, / for commands)"}
                // Read-only rather than disabled while sending: a disabled
                // field loses the focus, and the user would have to click
                // back into it for the next message.
                readOnly={isSending}
                aria-busy={isSending}
                role="combobox"
                aria-expanded={suggestions.length > 0}
                aria-autocomplete="list"
                className="flex-1 min-h-10.5 max-h-[33vh] p-2 border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
              />
            </div>
            <button
              type="submit"
              // Enabled while an agent answers too (issue 202): the runtime
              // takes a message at any time, and an agent in a turn hears it
              // once the turn ends.
              disabled={isSending || !inputText.trim()}
              className="h-10.5 p-2 bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white rounded-md transition-colors"
            >
              <Send size={20} />
            </button>
            {isGenerating && (
              <button
                type="button"
                onClick={handleStopGeneration}
                className="h-10.5 p-2 bg-red-600 hover:bg-red-700 text-white rounded-md transition-colors"
                title="Stop generation"
              >
                <Square size={20} fill="currentColor" />
              </button>
            )}
          </form>
          )}
        </div>
      </div>

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
          participant={historyOf.participant}
          me={me}
          participants={participants}
          version={messages.length}
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
          <div className="flex items-center gap-1 mb-4 text-gray-500 dark:text-gray-400" role="tablist">
            <button
              role="tab"
              aria-selected={sideTab === 'channel'}
              onClick={() => setSideTab('channel')}
              className={cn("flex items-center gap-1.5 px-2 py-1 rounded-md", sideTab === 'channel' && "bg-gray-100 dark:bg-gray-800 text-gray-800 dark:text-gray-100")}
            >
              <MessageSquare size={16} />
              <h3 className="font-semibold uppercase text-xs tracking-wider">Channel</h3>
            </button>
            <button
              role="tab"
              aria-selected={sideTab === 'notes'}
              onClick={() => setSideTab('notes')}
              className={cn("flex items-center gap-1.5 px-2 py-1 rounded-md", sideTab === 'notes' && "bg-gray-100 dark:bg-gray-800 text-gray-800 dark:text-gray-100")}
            >
              <NotebookPen size={16} />
              <h3 className="font-semibold uppercase text-xs tracking-wider">Notes</h3>
            </button>
            <button
              role="tab"
              aria-selected={sideTab === 'workspace'}
              onClick={() => setSideTab('workspace')}
              className={cn("flex items-center gap-1.5 px-2 py-1 rounded-md", sideTab === 'workspace' && "bg-gray-100 dark:bg-gray-800 text-gray-800 dark:text-gray-100")}
            >
              <HardDrive size={16} />
              <h3 className="font-semibold uppercase text-xs tracking-wider">Files</h3>
            </button>
          </div>
          {sideTab === 'notes' ? <NotesList notes={notes} /> : sideTab === 'workspace' ? (
            <ChannelWorkspacePanel workspace={workspace} files={workspaceFiles} version={workspaceVersion} />
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
              <span className="text-gray-700 dark:text-gray-300">{languageLabel(channel.language)}</span>
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
  );
};
