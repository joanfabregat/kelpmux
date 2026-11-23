// Loaded only when the relay configured a GlitchTip DSN (see ./errors.ts).
// A bare BrowserClient rather than `init`: `init` brings the default
// integrations (breadcrumbs, sessions, browser API wrapping, HTTP context),
// which the privacy rules exclude and which would also grow the bundle.
import {
  BrowserClient,
  dedupeIntegration,
  defaultStackParser,
  globalHandlersIntegration,
  linkedErrorsIntegration,
  makeFetchTransport,
  setCurrentClient,
} from "@sentry/browser";
import { scrubEvent, type ErrorReporting } from "./errors";

export const RELEASE =
  typeof __KELPMUX_RELEASE__ === "string" ? __KELPMUX_RELEASE__ : undefined;

export function start({ dsn, environment }: ErrorReporting) {
  const client = new BrowserClient({
    dsn,
    release: RELEASE,
    environment,
    transport: makeFetchTransport,
    stackParser: defaultStackParser,
    // Uncaught errors and unhandled rejections, their `cause` chain, and no
    // duplicate reports of one error.
    integrations: [
      globalHandlersIntegration(),
      linkedErrorsIntegration(),
      dedupeIntegration(),
    ],
    // No personal or page data (the SDK's successor to sendDefaultPii).
    dataCollection: {
      userInfo: false,
      cookies: false,
      httpHeaders: false,
      httpBodies: [],
      urlQueryParams: false,
      graphQL: { document: false, variables: false },
      genAI: { inputs: false, outputs: false },
      mcp: { inputs: false, outputs: false },
    },
    maxBreadcrumbs: 0,
    beforeBreadcrumb: () => null,
    sendClientReports: false,
    enhanceFetchErrorMessages: false,
    // `initialScope` only applies through `init`; tag each event here.
    beforeSend: (event) => {
      event.tags = { ...event.tags, component: "web" };
      return scrubEvent(event);
    },
  });
  setCurrentClient(client);
  client.init();
}
