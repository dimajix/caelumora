// What a channel's classification means for the user (issue 133): how
// sensitive its contents are, and for whom - its compartments.

/** The levels of a classification, from the most open. */
export type ConfidentialityLevel = 'public' | 'internal' | 'confidential' | 'strictly_confidential';

/**
 * A classification: a level and an audience of compartments. Without
 * compartments it is general; `public` takes none.
 */
export interface Classification {
  level: ConfidentialityLevel;
  /** Absent means none. */
  compartments?: string[];
}

interface ConfidentialityInfo {
  label: string;
  description: string;
}

const LEVELS: Record<ConfidentialityLevel, ConfidentialityInfo> = {
  public: {label: 'Public', description: 'Anyone may read what is said here.'},
  internal: {label: 'Internal', description: 'For the organisation: channels of the same or a higher level may read it.'},
  confidential: {label: 'Confidential', description: 'Only confidential and strictly confidential channels may read it.'},
  strictly_confidential: {label: 'Strictly confidential', description: 'Only strictly confidential channels may read it.'},
};

/** The levels, from the most open, for choosing one. */
export const CONFIDENTIALITY_LEVELS = Object.keys(LEVELS) as ConfidentialityLevel[];

/** The classification of a channel whose creator names none - the server's default. */
export const DEFAULT_CLASSIFICATION: Classification = {level: 'internal'};

/** The label and explanation of a level; one this UI does not know is shown by its name. */
export function confidentialityInfo(level: string): ConfidentialityInfo {
  return LEVELS[level as ConfidentialityLevel] ?? {label: level, description: `Confidentiality "${level}".`};
}

/** A classification for display, e.g. `Internal · hr, finance`. */
export function classificationLabel(classification: Classification): string {
  const level = confidentialityInfo(classification.level).label;
  const compartments = classification.compartments ?? [];
  return compartments.length === 0 ? level : `${level} · ${compartments.join(', ')}`;
}

/** The label of `classification` where it differs from `of` - a message's from its channel's -, else `null`. */
export function labelOtherThan(classification: Classification, of: Classification): string | null {
  return sameClassification(classification, of) ? null : classificationLabel(classification);
}

/** What a classification means, for a tooltip. */
export function classificationDescription(classification: Classification): string {
  const level = confidentialityInfo(classification.level).description;
  const compartments = classification.compartments ?? [];
  return compartments.length === 0
    ? `${level} No compartment: channels with a compartment may read it too.`
    : `${level} Meant for ${compartments.join(', ')}: only channels whose every compartment may read one of these.`;
}

/** Whether two classifications are the same. */
export function sameClassification(a: Classification, b: Classification): boolean {
  const names = (c: Classification) => [...(c.compartments ?? [])].sort().join('\n');
  return a.level === b.level && names(a) === names(b);
}

/**
 * The levels a message in a channel of `level` may have: the channel's and
 * those below - a message is never classified above its channel (issue 133).
 */
export function levelsUpTo(level: ConfidentialityLevel): ConfidentialityLevel[] {
  return CONFIDENTIALITY_LEVELS.slice(0, CONFIDENTIALITY_LEVELS.indexOf(level) + 1);
}

/**
 * Whether going from `from` to `to` releases something: a lower level, a
 * compartment added to the audience, or none left where there were some -
 * readable by more than before (issue 133).
 */
export function releases(from: Classification, to: Classification): boolean {
  const levels = CONFIDENTIALITY_LEVELS;
  if (levels.indexOf(to.level) < levels.indexOf(from.level)) return true;
  const before = new Set(from.compartments ?? []);
  const after = to.compartments ?? [];
  if (before.size > 0 && after.length === 0) return true;
  return after.some(c => !before.has(c));
}
