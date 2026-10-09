// A text field that grows with its text (issue 274): it starts as one line,
// becomes as tall as the text needs, and scrolls once it reaches the
// `max-height` its caller gives it.

import React, {useEffect, useLayoutEffect, useRef} from 'react';
import {cn} from '../classNames';

type GrowingTextareaProps = Omit<React.ComponentProps<'textarea'>, 'ref' | 'rows'> & {
  value: string;
  ref?: React.RefObject<HTMLTextAreaElement | null>;
};

/** Makes `field` as tall as its text needs; its `max-height` caps it. */
function fit(field: HTMLTextAreaElement) {
  field.style.height = 'auto';
  // `scrollHeight` leaves out the border, which the height includes - taken
  // from the style, since it may be a fraction of a pixel.
  const style = getComputedStyle(field);
  const border = parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
  field.style.height = `${field.scrollHeight + border}px`;
}

/** A `<textarea>` of one line that grows with its `value`, up to its `max-height`. */
export const GrowingTextarea: React.FC<GrowingTextareaProps> = ({ref: given, value, className, ...props}) => {
  const own = useRef<HTMLTextAreaElement>(null);
  const ref = given ?? own;

  useLayoutEffect(() => {
    if (ref.current) fit(ref.current);
  }, [ref, value]);

  // A change of width wraps the text anew - a resized window or panel.
  useEffect(() => {
    const field = ref.current;
    if (!field) return;
    let width = field.clientWidth;
    const observer = new ResizeObserver(() => {
      if (field.clientWidth === width) return;
      width = field.clientWidth;
      fit(field);
    });
    observer.observe(field);
    return () => observer.disconnect();
  }, [ref]);

  return (
    <textarea
      ref={ref}
      rows={1}
      value={value}
      className={cn('resize-none overflow-y-auto', className)}
      {...props}
    />
  );
};
