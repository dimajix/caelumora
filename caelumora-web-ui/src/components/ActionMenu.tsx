// The actions on something in a list - a session, a channel, a participant of a
// channel (issue 230): one button with three dots, opening a menu of what
// may be done with it, so that the actions take no room beside its name.

import React, {useEffect, useId, useState} from 'react';
import {createPortal} from 'react-dom';
import {MoreVertical} from 'lucide-react';
import {cn} from '../classNames';
import {useFloating} from './floating';

/** One entry of an `ActionMenu`. */
export interface MenuAction {
  label: string;
  icon: React.ComponentType<{size?: number; className?: string}>;
  onSelect: () => void;
  /** Shown in red: it removes or deletes something. */
  danger?: boolean;
  /**
   * Asked when the entry is first chosen, in its place, e.g. `Delete agent?`;
   * the action is taken only when it is chosen again.
   */
  confirm?: string;
  /** Explains the entry on hover. */
  title?: string;
}

interface ActionMenuProps {
  /** What the button is called for screen readers, e.g. `Options of Nova`. */
  label: string;
  /** The entries; without any there is no button. */
  actions: MenuAction[];
  /** Classes of the button, e.g. to show it only while its row is hovered. */
  className?: string;
  /** The size of the dots. */
  size?: number;
}

const PLACEMENT = {prefer: 'below', align: 'end'} as const;

export const ActionMenu: React.FC<ActionMenuProps> = ({label, actions, className, size = 14}) => {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [asking, setAsking] = useState<number | null>(null);
  const close = () => {
    setOpen(false);
    setAsking(null);
  };
  const {trigger: button, floating: menu, position} = useFloating<HTMLButtonElement, HTMLDivElement>(open, PLACEMENT, close);

  // A click anywhere else closes it.
  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!menu.current?.contains(target) && !button.current?.contains(target)) close();
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open, button, menu]);

  // Once placed, the keyboard is in the menu.
  useEffect(() => {
    if (position) menu.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
  }, [position, menu]);

  if (actions.length === 0) return null;

  const closeAndReturn = () => {
    close();
    button.current?.focus();
  };

  const choose = (index: number) => {
    const action = actions[index];
    if (action.confirm && asking !== index) {
      setAsking(index);
      return;
    }
    close();
    action.onSelect();
  };

  const onMenuKeyDown = (e: React.KeyboardEvent) => {
    const items = Array.from(menu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    const at = items.indexOf(document.activeElement as HTMLElement);
    const move = (to: number) => {
      e.preventDefault();
      items[(to + items.length) % items.length]?.focus();
    };
    switch (e.key) {
      case 'ArrowDown': move(at + 1); break;
      case 'ArrowUp': move(at - 1); break;
      case 'Home': move(0); break;
      case 'End': move(items.length - 1); break;
      case 'Escape': e.preventDefault(); closeAndReturn(); break;
      case 'Tab': close(); break;
    }
  };

  return (
    <>
      <button
        ref={button}
        type="button"
        onClick={(e) => {
          // The row it sits in is not chosen by it.
          e.stopPropagation();
          if (open) close(); else setOpen(true);
        }}
        className={cn('shrink-0 p-1 rounded-md aria-expanded:opacity-100', className)}
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
      >
        <MoreVertical size={size} />
      </button>
      {open && createPortal(
        <div
          ref={menu}
          id={id}
          role="menu"
          aria-label={label}
          onKeyDown={onMenuKeyDown}
          onClick={(e) => e.stopPropagation()}
          className={cn(
            'fixed z-50 min-w-36 max-w-[min(20rem,calc(100vw-1rem))] py-1 rounded-md shadow-lg',
            'border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 text-gray-700 dark:text-gray-200',
            position ? 'visible' : 'invisible',
          )}
          style={position ?? {left: 0, top: 0}}
        >
          {actions.map((action, index) => {
            const Icon = action.icon;
            return (
              <button
                key={action.label}
                type="button"
                role="menuitem"
                tabIndex={-1}
                onClick={() => choose(index)}
                title={action.title}
                className={cn(
                  'w-full flex items-center gap-2 px-3 py-2 text-xs text-left whitespace-nowrap',
                  'hover:bg-gray-100 dark:hover:bg-gray-700 focus:bg-gray-100 dark:focus:bg-gray-700 focus:outline-hidden',
                  action.danger && 'text-red-600 dark:text-red-400',
                  asking === index && 'font-semibold',
                )}
              >
                <Icon size={12} className="shrink-0" />
                <span>{asking === index ? action.confirm : action.label}</span>
              </button>
            );
          })}
        </div>,
        document.body,
      )}
    </>
  );
};
