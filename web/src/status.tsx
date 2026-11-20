import { icons, type IconName } from "./icons";

export function Icon({
  name,
  size,
  className = "",
}: {
  name: IconName;
  size?: "sm" | "lg";
  className?: string;
}) {
  return (
    <svg
      className={`svg-icon ${size || ""} ${className}`}
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
      // Static geometry vendored in icons.ts; never user input.
      dangerouslySetInnerHTML={{ __html: icons[name] }}
    />
  );
}
