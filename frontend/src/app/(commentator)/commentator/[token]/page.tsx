import CommentatorClient from "@/components/CommentatorClient";

interface Props {
  params: Promise<{ token: string }>;
}

export default async function CommentatorPage({ params }: Props) {
  const { token } = await params;
  return (
    <main className="commentator-page">
      <CommentatorClient token={token} />
    </main>
  );
}
