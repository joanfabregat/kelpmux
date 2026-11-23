/** A confirmation: its question and the actions offered besides Cancel. */
export type ConfirmAction = {
  title: string;
  text: string;
  buttons: {
    label: string;
    action: () => unknown | Promise<unknown>;
    color?: string;
  }[];
};

/** The confirmation dialog (Enter runs the first action and Escape cancels;
 * main.tsx handles both). */
export function ConfirmDialog({
  action,
  pending,
  onCancel,
  onApprove,
}: {
  action: ConfirmAction;
  pending: boolean;
  onCancel: () => void;
  onApprove: (index: number) => void;
}) {
  return (
    <div className="modal-shade confirmation">
      <section className="confirm-dialog" role="alertdialog">
        <h2>{action.title}</h2>
        <p>{action.text}</p>
        <div className="confirm-buttons">
          <button className="subtle" disabled={pending} onClick={onCancel}>
            Cancel
          </button>
          {action.buttons.map((b, index) => (
            <button
              key={b.label}
              className={b.color || ""}
              disabled={pending}
              onClick={() => onApprove(index)}
            >
              {b.label}
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}
