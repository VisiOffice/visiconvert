import type { SVGProps } from "react";
type Props = SVGProps<SVGSVGElement> & { size?: number };
const icon = (path: string) => (props: Props) => (
  <svg
    width={props.size || 20}
    height={props.size || 20}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.7"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
    {...props}
  >
    <path d={path} />
  </svg>
);
export const ArrowRight = icon("M4 12h16m-6-6 6 6-6 6");
export const ArrowDownLeft = icon("M18 6 6 18M6 7v11h11");
export const ChevronRight = icon("m9 5 7 7-7 7");
export const Check = icon("m5 12 4 4L19 6");
export const X = icon("m6 6 12 12M6 18 18 6");
export const Plus = icon("M12 5v14M5 12h14");
export const Layers3 = icon(
  "m12 3 10 5-10 5L2 8l10-5ZM2 12l10 5 10-5M2 16l10 5 10-5",
);
export const BarChart3 = icon("M4 3v18h17M8 16v-5m5 5V6m5 10v-8");
export const Search = icon("M21 21l-5-5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0");
export const Phone = icon(
  "M5 3h4l2 5-3 2a15 15 0 0 0 6 6l2-3 5 2v4a2 2 0 0 1-2 2C9 21 3 15 3 5a2 2 0 0 1 2-2Z",
);
export const Users = icon(
  "M16 21v-2a5 5 0 0 0-5-5H7a5 5 0 0 0-5 5v2M13 6a4 4 0 1 1-8 0 4 4 0 0 1 8 0M18 3a4 4 0 0 1 0 8m4 10v-2a5 5 0 0 0-4-5",
);
export const Link2 = icon(
  "m10 13 4-4M8 16l-1 1a4 4 0 0 1-6-6l5-5a4 4 0 0 1 6 0m0 12a4 4 0 0 0 6 0l5-5a4 4 0 0 0-6-6l-1 1",
);
export const ShieldCheck = icon(
  "m12 2 9 4v6c0 5-9 10-9 10S3 17 3 12V6l9-4Zm-5 10 3 3 7-7",
);
export const Sparkles = icon(
  "m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5L12 3Z",
);
export const Filter = icon("M3 5h18l-7 8v7l-4-2v-5L3 5Z");
export const LogOut = icon("M9 3H3v18h6m6-15 6 6-6 6M7 12h14");
export const Loader2 = icon("M21 12a9 9 0 1 1-9-9");
