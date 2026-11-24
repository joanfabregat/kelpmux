import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Icon } from "./status";
type Options = { tone?: "success" | "error"; key?: string };
type Toast = Options & { id: number; message: string };
const Context = createContext<(message: string, options?: Options) => void>(
  () => {},
);
export const useToast = () => useContext(Context);
/** The confirmations shown by `useToast`, in the top right stack, newest
 * first, below `aside` (the running downloads). The attention toasts sit
 * above the stack (dialogs.css). */
export function ToastProvider({
  children,
  aside,
}: {
  children: ReactNode;
  aside?: ReactNode;
}) {
  const [items, setItems] = useState<Toast[]>([]);
  const next = useRef(0);
  const show = useCallback((message: string, options: Options = {}) => {
    const item = { ...options, message, id: ++next.current };
    setItems((old) =>
      [...old.filter((t) => !options.key || t.key !== options.key), item].slice(
        -3,
      ),
    );
  }, []);
  const dismiss = useCallback(
    (id: number) => setItems((old) => old.filter((t) => t.id !== id)),
    [],
  );
  return (
    <Context.Provider value={show}>
      {children}
      <div className="toast-stack">
        {aside}
        <div
          className="toast-region"
          role="status"
          aria-live="polite"
          aria-label="Action confirmations"
        >
          {items
            .slice()
            .reverse()
            .map((item) => (
              <ToastItem key={item.id} item={item} dismiss={dismiss} />
            ))}
        </div>
      </div>
    </Context.Provider>
  );
}
function ToastItem({
  item,
  dismiss,
}: {
  item: Toast;
  dismiss: (id: number) => void;
}) {
  const remaining = useRef(item.tone === "error" ? 10000 : 4000);
  const [hover, setHover] = useState(false),
    [focus, setFocus] = useState(false);
  useEffect(() => {
    if (hover || focus) return;
    const started = Date.now();
    const timer = setTimeout(() => dismiss(item.id), remaining.current);
    return () => {
      clearTimeout(timer);
      remaining.current = Math.max(
        0,
        remaining.current - (Date.now() - started),
      );
    };
  }, [hover, focus, item.id, dismiss]);
  return (
    <div
      className={`toast toast-${item.tone || "success"}`}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onFocus={() => setFocus(true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setFocus(false);
      }}
    >
      <Icon
        name={item.tone === "error" ? "circle-alert" : "circle-check"}
        className="toast-icon"
      />
      <span>{item.message}</span>
      <button
        className="icon-btn"
        title="Dismiss confirmation"
        aria-label="Dismiss confirmation"
        onPointerDown={(event) => event.preventDefault()}
        onClick={() => dismiss(item.id)}
      >
        <Icon name="x" size="sm" />
      </button>
    </div>
  );
}
