// Display metadata only. It never participates in WebAuthn or SSH identity.
export const keyColors: Record<string, string> = {
  blue: "#5fafff",
  sage: "#83b9aa",
  glacier: "#79bec8",
  slate: "#a3abc8",
  sand: "#c6ad7e",
  rose: "#ce9299",
};
export const keyEmojis = [
  { value: "", label: "Key" },
  { value: "🔑", label: "Key emoji" },
  { value: "🖥️", label: "Desktop" },
  { value: "💻", label: "Laptop" },
  { value: "🏠", label: "Home" },
  { value: "🏢", label: "Office" },
  { value: "🧳", label: "Travel" },
  { value: "🛡️", label: "Shield" },
  { value: "🌿", label: "Leaf" },
  { value: "🌊", label: "Ocean" },
  { value: "⭐", label: "Star" },
  { value: "🚀", label: "Rocket" },
  { value: "🐸", label: "Frog" },
  { value: "🐙", label: "Octopus" },
];
export const keyColor = (color?: string) =>
  keyColors[color || ""] || keyColors.blue;
