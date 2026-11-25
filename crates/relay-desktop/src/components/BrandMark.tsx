import { useId } from "react";

interface BrandMarkProps {
  className?: string;
}

export function BrandMark({ className }: BrandMarkProps) {
  const uid = useId().replace(/:/g, "");
  const bgId = `${uid}-bg`;
  const sheenId = `${uid}-sheen`;
  const rimId = `${uid}-rim`;
  const silverId = `${uid}-silver`;
  const tealId = `${uid}-teal`;

  return (
    <svg aria-hidden="true" viewBox="0 0 512 512" fill="none" className={className}>
      <defs>
        <linearGradient id={bgId} x1="48" y1="32" x2="464" y2="480" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stopColor="#252E3D" />
          <stop offset="48%" stopColor="#151B25" />
          <stop offset="100%" stopColor="#0C1017" />
        </linearGradient>
        <radialGradient id={sheenId} cx="30%" cy="18%" r="65%">
          <stop offset="0%" stopColor="#FFFFFF" stopOpacity="0.1" />
          <stop offset="100%" stopColor="#FFFFFF" stopOpacity="0" />
        </radialGradient>
        <linearGradient
          id={rimId}
          x1="256"
          y1="16"
          x2="256"
          y2="496"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="#FFFFFF" stopOpacity="0.24" />
          <stop offset="45%" stopColor="#FFFFFF" stopOpacity="0.06" />
          <stop offset="100%" stopColor="#FFFFFF" stopOpacity="0.02" />
        </linearGradient>
        <linearGradient
          id={silverId}
          x1="140"
          y1="104"
          x2="368"
          y2="336"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="#FFFFFF" />
          <stop offset="65%" stopColor="#E2E8F0" />
          <stop offset="100%" stopColor="#CBD5E1" />
        </linearGradient>
        <linearGradient
          id={tealId}
          x1="152"
          y1="396"
          x2="368"
          y2="180"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0%" stopColor="#0D9488" />
          <stop offset="55%" stopColor="#14B8A6" />
          <stop offset="100%" stopColor="#34D399" />
        </linearGradient>
      </defs>

      <rect x="16" y="16" width="480" height="480" rx="112" fill={`url(#${bgId})`} />
      <rect x="16" y="16" width="480" height="480" rx="112" fill={`url(#${sheenId})`} />
      <rect
        x="18"
        y="18"
        width="476"
        height="476"
        rx="110"
        stroke={`url(#${rimId})`}
        strokeWidth="3.5"
      />

      <path
        d="M 346 156 A 104 104 0 1 0 256 312 L 264 312"
        stroke={`url(#${silverId})`}
        strokeWidth="40"
        strokeLinecap="butt"
      />
      <circle cx="346" cy="156" r="32" fill={`url(#${silverId})`} />
      <path d="M 248 264 L 304 312 L 248 360 Z" fill={`url(#${silverId})`} />

      <g transform="rotate(180 256 256)">
        <path
          d="M 346 156 A 104 104 0 1 0 256 312 L 264 312"
          stroke={`url(#${tealId})`}
          strokeWidth="40"
          strokeLinecap="butt"
        />
        <circle cx="346" cy="156" r="32" fill={`url(#${tealId})`} />
        <path d="M 248 264 L 304 312 L 248 360 Z" fill={`url(#${tealId})`} />
      </g>
    </svg>
  );
}
