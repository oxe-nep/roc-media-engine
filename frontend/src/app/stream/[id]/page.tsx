import HlsPlayer from "@/components/HlsPlayer";
import Link from "next/link";

const BASE = process.env.NEXT_PUBLIC_BACKEND_URL ?? "http://localhost:8080";

interface Props {
  params: Promise<{ id: string }>;
}

export default async function StreamPage({ params }: Props) {
  const { id } = await params;
  const hlsSrc = `${BASE}/hls/${id}/index.m3u8`;

  return (
    <main className="min-h-screen bg-slate-950 text-white p-6">
      <Link href="/" className="text-slate-400 hover:text-white text-sm mb-6 inline-block">
        ← Back
      </Link>
      <h1 className="text-xl font-bold mb-4">Channel {id}</h1>
      <div className="max-w-4xl">
        <HlsPlayer
          src={hlsSrc}
          className="w-full rounded-xl bg-slate-900 aspect-video"
        />
      </div>
    </main>
  );
}
