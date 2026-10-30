import {
  Target,
  Zap,
  Radio,
  ShieldAlert,
  Settings,
  Boxes,
  Database,
  ArrowLeftRight,
  ScrollText,
  Network,
  CandlestickChart,
  type LucideIcon,
} from 'lucide-react';

// Maps the string icon names used in data files to Lucide components, so
// content can stay as plain serializable data.
const map: Record<string, LucideIcon> = {
  Target,
  Zap,
  Radio,
  ShieldAlert,
  Settings,
  Boxes,
  Database,
  ArrowLeftRight,
  ScrollText,
  Network,
  CandlestickChart,
};

export default function Icon({
  name,
  size = 24,
  className = '',
}: {
  name: string;
  size?: number;
  className?: string;
}) {
  const Cmp = map[name] ?? Target;
  return <Cmp size={size} className={className} aria-hidden="true" />;
}
