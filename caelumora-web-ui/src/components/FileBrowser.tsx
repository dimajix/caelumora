// The files of a workspace (issue 253), reached one way or the other -
// directly by its owner, or through a channel (`WorkspaceFiles`): one
// directory at a time with a path back to the top, downloads as plain
// links, uploads into the directory shown, deleting with a second click.
// What may be done is the server's to decide; a refusal is reported.

import React, {useEffect, useRef, useState} from 'react';
import {ChevronRight, File, Folder, RefreshCw, Trash2, Upload} from 'lucide-react';
import type {FileEntry} from '../types';
import type {WorkspaceFiles} from '../api';
import {reportError} from '../errors';
import {formatSize, formatTimestamp} from '../messages';
import {ActionMenu} from './ActionMenu';

/** `name` in the directory `dir`; `''` is the workspace itself. */
function join(dir: string, name: string): string {
  return dir === '' ? name : `${dir}/${name}`;
}

interface FileBrowserProps {
  files: WorkspaceFiles;
  /** Changes whenever the files may have changed elsewhere - after a tool call, say. */
  version?: number;
}

export const FileBrowser: React.FC<FileBrowserProps> = ({files, version = 0}) => {
  const [dir, setDir] = useState('');
  const [entries, setEntries] = useState<FileEntry[] | null>(null);
  const [reload, setReload] = useState(0);
  const [isUploading, setIsUploading] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let current = true;
    files.list(dir)
      .then(found => { if (current) setEntries(found); })
      .catch(error => {
        if (!current) return;
        reportError('Failed to list the files', error);
        setEntries([]);
      });
    return () => { current = false; };
  }, [files, dir, version, reload]);

  const upload = async (chosen: FileList | null) => {
    if (!chosen || chosen.length === 0) return;
    setIsUploading(true);
    try {
      // One after the other, each into the directory shown.
      for (const file of Array.from(chosen)) {
        await files.upload(join(dir, file.name), file);
      }
    } catch (error) {
      reportError('Failed to upload', error);
    } finally {
      setIsUploading(false);
      if (input.current) input.current.value = '';
      setReload(r => r + 1);
    }
  };

  const remove = async (path: string) => {
    try {
      await files.remove(path);
    } catch (error) {
      reportError(`Failed to delete ${path}`, error);
    }
    setReload(r => r + 1);
  };

  const parts = dir === '' ? [] : dir.split('/');
  // Directories first, each group by name.
  const sorted = [...(entries ?? [])].sort((l, r) => Number(r.is_dir) - Number(l.is_dir) || l.name.localeCompare(r.name));

  return (
    <div className="text-sm">
      <div className="flex items-center gap-1 mb-2 text-xs text-gray-500 dark:text-gray-400">
        <nav className="flex-1 min-w-0 flex items-center flex-wrap" aria-label="Directory">
          <button onClick={() => setDir('')} className="hover:underline">workspace</button>
          {parts.map((part, index) => (
            <React.Fragment key={index}>
              <ChevronRight size={12} className="shrink-0" />
              <button onClick={() => setDir(parts.slice(0, index + 1).join('/'))} className="hover:underline truncate">{part}</button>
            </React.Fragment>
          ))}
        </nav>
        <button
          onClick={() => setReload(r => r + 1)}
          className="p-1 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800"
          title="Reload"
          aria-label="Reload the files"
        >
          <RefreshCw size={12} />
        </button>
        <button
          onClick={() => input.current?.click()}
          disabled={isUploading}
          className="flex items-center gap-1 px-2 py-1 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 disabled:opacity-50"
          title={dir === '' ? 'Upload files into the workspace' : `Upload files into ${dir}`}
        >
          <Upload size={12} />
          {isUploading ? 'Uploading…' : 'Upload'}
        </button>
        <input ref={input} type="file" multiple className="hidden" onChange={(e) => void upload(e.target.files)} />
      </div>

      {entries === null ? null : sorted.length === 0 ? (
        <div className="text-xs italic text-gray-500">{dir === '' ? 'The workspace is empty.' : 'The directory is empty.'}</div>
      ) : (
        <ul className="space-y-0.5">
          {sorted.map(entry => {
            const path = join(dir, entry.name);
            const modified = formatTimestamp(entry.modified);
            return (
              <li key={entry.name} className="group/file flex items-center gap-2 rounded-md px-1 hover:bg-gray-100 dark:hover:bg-gray-800">
                {entry.is_dir ? (
                  <button onClick={() => setDir(path)} className="flex-1 min-w-0 flex items-center gap-2 py-1 text-left">
                    <Folder size={14} className="shrink-0 text-gray-400" />
                    <span className="truncate">{entry.name}</span>
                  </button>
                ) : (
                  <a href={files.url(path)} download={entry.name} className="flex-1 min-w-0 flex items-center gap-2 py-1 hover:underline" title={`Download ${entry.name}`}>
                    <File size={14} className="shrink-0 text-gray-400" />
                    <span className="truncate">{entry.name}</span>
                  </a>
                )}
                {!entry.is_dir && <span className="shrink-0 text-xs text-gray-500">{formatSize(entry.size)}</span>}
                {modified && <span className="shrink-0 text-xs text-gray-500 hidden lg:inline" title={modified.full}>{modified.short}</span>}
                <ActionMenu
                  label={`Options of ${entry.name}`}
                  className="p-1 opacity-0 group-hover/file:opacity-100 focus:opacity-100 transition-opacity"
                  actions={[
                    {label: 'Delete', icon: Trash2, danger: true, confirm: entry.is_dir ? 'Delete directory?' : 'Delete file?', onSelect: () => void remove(path)},
                  ]}
                />
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
};
