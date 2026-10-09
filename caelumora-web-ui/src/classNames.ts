// Composing Tailwind classes, for every component alike.

import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';

/**
 * Joins the classes given, leaving out the false ones; of two Tailwind
 * classes that set the same thing the later one wins, so a `className` handed
 * in by a caller overrides the component's own.
 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
