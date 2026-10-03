import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "ROC Recording",
  description: "Blackmagic DeckLink preview",
  icons: {
    icon: "https://cdn.prod.website-files.com/5bc9fe82c6c2f54b071f0033/5bc9fe82c6c2f5193f1f01c5_Untitled-4.png",
  },
};

interface LayoutProps {
  children: React.ReactNode;
}

export default function RootLayout({ children }: LayoutProps) {
  return (
    <html lang="en">
      <head>
        <link
          href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css"
          rel="stylesheet"
        />
      </head>
      <body>
        <div className="container">{children}</div>
      </body>
    </html>
  );
}
