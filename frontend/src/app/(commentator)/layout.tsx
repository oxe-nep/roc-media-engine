import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Commentator",
  description: "NEP remote commentator",
};

export default function CommentatorLayout({ children }: { children: React.ReactNode }) {
  return <div className="commentator-root">{children}</div>;
}
