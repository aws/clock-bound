interface GridBackgroundProps {
  className?: string;
}

// Decorative subtle grid with a radial fade mask. Purely presentational, so
// it is hidden from assistive tech.
export default function GridBackground({ className = '' }: GridBackgroundProps) {
  return (
    <div
      aria-hidden="true"
      className={`pointer-events-none absolute inset-0 -z-10 bg-grid-slate bg-[size:40px_40px] [mask-image:radial-gradient(50rem_40rem_at_50%_0%,black,transparent)] ${className}`}
    />
  );
}
