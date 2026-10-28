// Errors from the REST API and how the UI reports them (issue 02).
//
// The server answers errors with RFC 9457 problem details; `ApiRequestError`
// carries what the user needs from them: the `detail` as message, and the
// `request_id` under which the full error is in the server log.

/** A problem details document as sent by the server. */
export interface ProblemDetails {
  type?: string;
  title?: string;
  status?: number;
  detail?: string;
  code?: string;
  request_id?: string;
}

/** A failed API request. */
export class ApiRequestError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly requestId?: string;

  constructor(message: string, status: number, code?: string, requestId?: string) {
    super(message);
    this.name = 'ApiRequestError';
    this.status = status;
    this.code = code;
    this.requestId = requestId;
  }
}

/**
 * Returns `res` if it succeeded; otherwise throws an `ApiRequestError` built
 * from its problem details, or from `fallback` if the body has none.
 */
export async function ensureOk(res: Response, fallback: string): Promise<Response> {
  if (res.ok) return res;
  let problem: ProblemDetails = {};
  try {
    problem = await res.json();
  } catch {
    // Not a problem details body, e.g. a proxy error page.
  }
  const requestId = problem.request_id ?? res.headers.get('x-request-id') ?? undefined;
  throw new ApiRequestError(problem.detail || fallback, res.status, problem.code, requestId);
}

/** An error as the UI shows it. */
export interface ReportedError {
  id: number;
  message: string;
  requestId?: string;
}

let reporter: ((error: ReportedError) => void) | null = null;
let nextId = 1;

/** Installs where reported errors are shown, e.g. the banner in `App`. */
export function setErrorReporter(handler: ((error: ReportedError) => void) | null) {
  reporter = handler;
}

/**
 * Reports a failed operation to the user. `action` says what was attempted
 * ("Failed to load sessions"); the server's detail, if any, follows it. A
 * `401` is not reported: it already returns the UI to the login page.
 */
export function reportError(action: string, error: unknown) {
  console.error(action, error);
  if (error instanceof ApiRequestError && error.status === 401) return;
  const detail = error instanceof Error ? error.message : String(error);
  const message = detail && detail !== action ? `${action}: ${detail}` : action;
  const requestId = error instanceof ApiRequestError ? error.requestId : undefined;
  reporter?.({ id: nextId++, message, requestId });
}

/**
 * Shows through `show` what the server refused of the request - the request's
 * fault, any status below 500 but a `401` -, so that a dialog shows it where it
 * can be corrected; reports anything else as `reportError` does.
 */
export function reportUnlessRefused(action: string, error: unknown, show: (refusal: string) => void) {
  if (error instanceof ApiRequestError && error.status < 500 && error.status !== 401) {
    show(error.message);
  } else {
    reportError(action, error);
  }
}
