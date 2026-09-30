// What a session's privacy policy means for the user (issue 134): whether
// other sessions may read what was said in it - for now, what an agent
// remembers here and recalls elsewhere.

/** The privacy policies of a session. */
export type PrivacyPolicy = 'private' | 'same_owner' | 'shared_audience';

interface PrivacyInfo {
  label: string;
  description: string;
}

const POLICIES: Record<PrivacyPolicy, PrivacyInfo> = {
  private: {
    label: 'Private',
    description: 'What is said here stays in this session.',
  },
  same_owner: {
    label: "Owner's sessions",
    description: "The owner's other sessions may read what is said here - whoever takes part in them.",
  },
  shared_audience: {
    label: 'Shared audience',
    description: 'Other sessions may read what is said here if everyone in them also takes part in this one.',
  },
};

/** The policies, the most closed first, for choosing one. */
export const PRIVACY_POLICIES = Object.keys(POLICIES) as PrivacyPolicy[];

/** The label and explanation of a policy; one this UI does not know is shown by its name. */
export function privacyInfo(policy: string): PrivacyInfo {
  return POLICIES[policy as PrivacyPolicy] ?? {label: policy, description: `Privacy "${policy}".`};
}
