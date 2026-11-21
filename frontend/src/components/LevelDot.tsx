import { LEVEL_COLORS, type LatencyLevel } from '../utils/latency';

/** The small coloured dot that carries a latency level beside a number. The
 *  number itself stays in the normal text colour: a page full of saturated
 *  green / amber / red figures reads as noise, a dot reads as status. */
export default function LevelDot({ level }: { level: LatencyLevel }) {
  return <span className="rp-lat-dot" aria-hidden="true" style={{ background: LEVEL_COLORS[level] }} />;
}
