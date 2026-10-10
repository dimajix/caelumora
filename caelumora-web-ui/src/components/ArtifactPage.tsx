// An artifact the user may read (issue 291), in the place of a channel: what
// it is - its name, where it lies, its classification and what its owner
// says of it - with what its owner may do to it, whom it is shared with as a
// whole (issue 23), and the versions the user may read, newest first: where
// each was published and by whom, as far as they may read that channel, its
// download, and for the owner whom the version reaches besides.

import React, {useEffect, useState} from 'react';
import {Download, FileText, Package, PackageCheck, Shield, Trash2} from 'lucide-react';
import type {AccessEntry, ArtifactResponse, ArtifactVersion} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {classificationDescription, classificationLabel} from '../confidentiality';
import {formatSize, formatTimestamp} from '../messages';
import {scopeLabel} from '../scope';
import {AccessList} from './AccessList';
import {ActionMenu} from './ActionMenu';

interface ArtifactPageProps {
  artifactId: string;
  onDeleted: (artifactId: string) => void;
  /** Opens the channel a version was published in. */
  onSelectChannel: (sessionId: string, channelId: string) => void;
}

/** The sidebar learns of a change from the global stream. */
export const ArtifactPage: React.FC<ArtifactPageProps> = ({artifactId, onDeleted, onSelectChannel}) => {
  const [artifact, setArtifact] = useState<ArtifactResponse | null>(null);
  const [isDescribing, setIsDescribing] = useState(false);
  const [text, setText] = useState('');

  useEffect(() => {
    let current = true;
    api.getArtifact(artifactId)
      .then(found => { if (current) setArtifact(found); })
      .catch(error => reportError('Failed to load the artifact', error));
    return () => { current = false; };
  }, [artifactId]);

  if (!artifact) return null;

  const describe = () => {
    setText(artifact.description ?? '');
    setIsDescribing(true);
  };

  const save = async () => {
    setIsDescribing(false);
    if (text.trim() === (artifact.description ?? '')) return;
    try {
      setArtifact(await api.describeArtifact(artifact.id, text.trim() || null));
    } catch (error) {
      reportError('Failed to describe the artifact', error);
    }
  };

  const keep = async (scope: 'user' | 'global') => {
    try {
      setArtifact(await api.keepArtifact(artifact.id, scope));
    } catch (error) {
      reportError('Failed to keep the artifact', error);
    }
  };

  const share = async (access: AccessEntry[]) => {
    try {
      setArtifact(await api.setArtifactAccess(artifact.id, access));
    } catch (error) {
      reportError('Failed to share the artifact', error);
    }
  };

  const remove = async () => {
    try {
      await api.deleteArtifact(artifact.id);
      onDeleted(artifact.id);
    } catch (error) {
      reportError('Failed to delete the artifact', error);
    }
  };

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <div className="flex items-start gap-3">
          <Package size={22} className="mt-1 shrink-0 text-gray-400" />
          <div className="flex-1 min-w-0 space-y-1">
            <h2 className="text-xl font-semibold truncate text-gray-900 dark:text-gray-100">{artifact.name}</h2>
            {isDescribing ? (
              <input
                autoFocus
                value={text}
                onChange={(e) => setText(e.target.value)}
                onBlur={() => void save()}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void save();
                  if (e.key === 'Escape') setIsDescribing(false);
                }}
                className="w-full rounded-md border border-blue-500 bg-white dark:bg-gray-800 px-2 py-1 text-sm"
              />
            ) : (
              <p className="text-sm text-gray-600 dark:text-gray-400">{artifact.description || <span className="italic">No description</span>}</p>
            )}
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-500 dark:text-gray-400">
              <span>Name space: {scopeLabel(artifact)}{artifact.scope === 'session' && ', gone with it unless kept'}</span>
              <span className="flex items-center gap-1" title={classificationDescription(artifact.classification)}>
                <Shield size={12} /> {classificationLabel(artifact.classification)}
              </span>
            </div>
          </div>
          {artifact.can_change && (
            <ActionMenu
              label={`Options of ${artifact.name}`}
              size={16}
              className="p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
              actions={[
                {label: 'Description', icon: FileText, onSelect: describe},
                ...(artifact.scope === 'session' ? [
                  {label: 'Keep among your artifacts', icon: PackageCheck, onSelect: () => void keep('user')},
                  {label: 'Keep for everyone', icon: PackageCheck, onSelect: () => void keep('global')},
                ] : []),
                {label: 'Delete', icon: Trash2, danger: true, confirm: 'Delete artifact?', onSelect: () => void remove()},
              ]}
            />
          )}
        </div>

        {artifact.can_change && (
          <AccessList
            access={artifact.access ?? []}
            onChange={(access) => void share(access)}
            nobody="Nobody as a whole: each version is read by the members of the channel it was published in."
            grants={{read: 'reads every version', write: 'reads and adds versions'}}
          />
        )}
        {!artifact.can_change && artifact.can_write && (
          <div className="text-xs italic text-gray-500">You may add versions to it.</div>
        )}

        <div className="space-y-2">
          <h3 className="text-sm font-medium text-gray-800 dark:text-gray-100">Versions</h3>
          <ul className="space-y-2">
            {[...artifact.versions].reverse().map(version => (
              <VersionRow key={version.number} artifact={artifact} version={version} onSelectChannel={onSelectChannel} />
            ))}
          </ul>
        </div>
      </div>
    </div>
  );
};

interface VersionRowProps {
  artifact: ArtifactResponse;
  version: ArtifactVersion;
  onSelectChannel: (sessionId: string, channelId: string) => void;
}

/**
 * One version: its number, size and what it is, when, where and by whom it
 * was published - the channel a link into it where the server names it -,
 * its download, and for the owner whom it reaches besides the artifact's
 * list. A version's own list is set when it is published; it is not
 * changed here.
 */
const VersionRow: React.FC<VersionRowProps> = ({artifact, version, onSelectChannel}) => {
  const at = formatTimestamp(version.published_at);
  const readers = version.access ?? [];
  return (
    <li className="rounded-md border border-gray-200 dark:border-gray-700 p-3 space-y-1">
      <div className="flex items-start gap-3">
        <div className="flex-1 min-w-0 space-y-1">
          <div className="text-sm font-medium text-gray-900 dark:text-gray-100">
            Version {version.number}
            <span className="ml-2 text-xs font-normal text-gray-500">
              {[formatSize(version.size), version.media_type].filter(Boolean).join(', ')}
            </span>
          </div>
          {version.description && <p className="text-sm text-gray-600 dark:text-gray-400">{version.description}</p>}
          <div className="text-xs text-gray-500 dark:text-gray-400">
            {version.channel_name !== undefined && version.session_name !== undefined ? (
              <>
                Published in{' '}
                <button
                  type="button"
                  onClick={() => onSelectChannel(version.session_id, version.channel_id)}
                  className="text-blue-600 dark:text-blue-400 hover:underline"
                  title="Open the channel"
                >
                  {version.session_name} / {version.channel_name}
                </button>
                {version.published_by_name && <> by {version.published_by_name}</>}
              </>
            ) : (
              <>Published in a channel you do not read</>
            )}
            {at && <span title={at.full}>, {at.short}</span>}
          </div>
          {readers.length > 0 && (
            <div className="text-xs text-gray-500 dark:text-gray-400">
              Also read by: {readers.map(entry => entry.name ?? 'members gone').join(', ')}
            </div>
          )}
        </div>
        <a
          href={api.artifactVersionUrl(artifact.id, version.number)}
          download={artifact.name}
          className="flex items-center gap-1 rounded-md bg-blue-600 hover:bg-blue-700 text-white text-xs px-2.5 py-1.5"
          title={`Download version ${version.number}`}
        >
          <Download size={12} /> Download
        </a>
      </div>
    </li>
  );
};
