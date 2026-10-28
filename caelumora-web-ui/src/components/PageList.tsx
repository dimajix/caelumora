// A list in the sidebar of what opens as a page in the place of a channel -
// workspaces (issue 253), projects (issue 467), skills (issue 22), artifacts
// (issue 291), teams (issue 286), agents (issue 227): those the user may read,
// each with where it lies where that is not their own scope,
// loaded again on every global event about one of them, with "+" to make
// one where one is made here, and what was deleted (issue 164) folded below.

import React from 'react';
import {Plus, type LucideIcon} from 'lucide-react';
import type {Deleted} from '../types';
import {scopeLabel} from '../scope';
import {isPageOf, type Page} from '../page';
import type {Listed, PageListState} from '../pageList';
import {DeletedList} from './DeletedList';
import {cn} from '../classNames';

interface PageListProps<T extends Listed> {
  list: PageListState<T>;
  /** The heading - "Workspaces". */
  title: string;
  icon: LucideIcon;
  page: Page | null;
  onSelectPage: (page: Page) => void;
  /** Makes one - opens its dialog; offered as "+" titled `newLabel`. */
  onNew?: () => void;
  newLabel?: string;
  /** What the user deleted, loaded again whenever `deletedVersion` changes. */
  loadDeleted: () => Promise<Deleted[]>;
  deletedVersion: number;
  restore: (id: string) => Promise<unknown>;
  purge: (id: string) => Promise<void>;
}

export function PageList<T extends Listed>({list, title, icon: Icon, page, onSelectPage, onNew, newLabel, loadDeleted, deletedVersion, restore, purge}: PageListProps<T>): React.ReactElement {
  return (
    <div className="mt-4 space-y-1">
      <div className="flex items-center px-2 text-xs uppercase tracking-wider text-gray-500">
        <span className="flex-1">{title}</span>
        {onNew && (
          <button
            onClick={onNew}
            className="p-1 hover:bg-gray-800 hover:text-white rounded-md transition-colors"
            title={newLabel}
            aria-label={newLabel}
          >
            <Plus size={14} />
          </button>
        )}
      </div>
      {list.items.map(item => (
        <button
          key={item.id}
          onClick={() => onSelectPage({kind: list.kind, id: item.id})}
          className={cn(
            "w-full flex items-center p-2 rounded-md hover:bg-gray-800 transition-colors text-sm text-gray-300",
            isPageOf(page, list.kind, item.id) && "bg-gray-800 text-white font-medium"
          )}
          title={item.description || (item.scope ? `${item.name} (${scopeLabel({scope: item.scope})})` : item.name)}
        >
          <Icon size={14} className="mr-2 shrink-0" />
          <span className="truncate">{item.name}</span>
          {item.scope && item.scope !== 'user' && <span className="ml-1 shrink-0 text-xs text-gray-500">{scopeLabel({scope: item.scope})}</span>}
        </button>
      ))}
      {list.items.length === 0 && (
        <div className="p-2 text-xs text-gray-500 italic">No {list.what}</div>
      )}
      <DeletedList
        title={`Deleted ${list.what}`}
        load={loadDeleted}
        version={deletedVersion}
        onRestore={async (id) => { await restore(id); list.reload(); }}
        onPurge={purge}
      />
    </div>
  );
}
