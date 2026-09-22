import React from 'react';
import {AlertTriangle, X} from 'lucide-react';
import type {ReportedError} from '../errors';

interface ErrorBannerProps {
  errors: ReportedError[];
  onDismiss: (id: number) => void;
}

/** Failed operations, newest last, each with the request id to look up in the server log. */
export const ErrorBanner: React.FC<ErrorBannerProps> = ({ errors, onDismiss }) => {
  if (errors.length === 0) return null;
  return (
    <div className="fixed top-3 left-1/2 -translate-x-1/2 z-50 w-full max-w-xl px-4 space-y-2">
      {errors.map(error => (
        <div
          key={error.id}
          role="alert"
          className="flex items-start gap-2 p-3 rounded-md shadow-lg border border-red-300 dark:border-red-800 bg-red-50 dark:bg-red-950 text-red-800 dark:text-red-200 text-sm"
        >
          <AlertTriangle size={16} className="mt-0.5 shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="break-words">{error.message}</div>
            {error.requestId && (
              <div className="mt-1 text-[10px] font-mono opacity-60">request_id {error.requestId}</div>
            )}
          </div>
          <button
            onClick={() => onDismiss(error.id)}
            className="p-0.5 rounded hover:bg-red-100 dark:hover:bg-red-900"
            title="Dismiss"
          >
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
};
