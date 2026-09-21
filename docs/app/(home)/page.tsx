import Link from 'next/link';

export default function HomePage() {
  return (
    <div className="flex flex-col justify-center text-center flex-1">
      <h1 className="text-2xl font-bold mb-4">Zeus</h1>
      <p className="text-fd-muted-foreground mb-4">
        Native macOS control plane for coding agents.
      </p>
      <p>
        Open the{' '}
        <Link href="/docs" className="font-medium underline">
          documentation
        </Link>
        .
      </p>
    </div>
  );
}
