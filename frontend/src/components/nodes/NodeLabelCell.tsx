import { Typography } from 'antd';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';

const { Text } = Typography;

const WIDE_STYLE = { display: 'inline-block', maxWidth: '100%', verticalAlign: 'bottom' } as const;

/** Truncate a long name with an ellipsis (its full text is the tooltip) — but
 *  never squeeze it below the room it needs: inside a flex row, as on the mobile
 *  card, `maxWidth` alone let it shrink to a bare "…". */
const CELL_STYLE = { display: 'inline-block', maxWidth: 150, minWidth: 48, verticalAlign: 'bottom' } as const;

/** The longest label the panel accepts (characters), kept in step with
 *  `MAX_LABEL_CHARS` in the backend's node_labels service. */
export const MAX_NODE_LABEL = 40;

interface Props {
  row: NodeDisplayRow;
  t: Tfn;
  /** The cell has a whole line to itself (the mobile card): let the name use it
   *  instead of stopping at the table column's width. */
  wide?: boolean;
  /** Admin only. Saves the new label ('' clears it); resolves true on success.
   *  Absent: the cell just shows the label, as a regular user sees it. */
  onSave?: (row: NodeDisplayRow, label: string) => Promise<boolean>;
}

/**
 * One machine's label. For an admin it is edited in place (pencil, Enter to
 * save, Esc to cancel); an empty label saves as "no label".
 *
 * A group's no-node placeholder row has no node id and so nothing to name.
 */
export function NodeLabelCell({ row, t, wide, onSave }: Props) {
  const label = row.label ?? '';
  const style = wide ? WIDE_STYLE : CELL_STYLE;
  if (!row.node_id) return <Text type="secondary">-</Text>;
  if (!onSave) {
    return label ? <Text ellipsis={{ tooltip: label }} style={style}>{label}</Text> : <Text type="secondary">-</Text>;
  }
  return (
    <Text
      className="rp-node-label"
      style={style}
      type={label ? undefined : 'secondary'}
      ellipsis={label ? { tooltip: label } : false}
      editable={{
        text: label,
        maxLength: MAX_NODE_LABEL,
        tooltip: t('nodeLabelEdit'),
        triggerType: ['icon'],
        // antd calls this on Enter/blur with the edited text.
        onChange: (next: string) => {
          if (next.trim() !== label) void onSave(row, next);
        },
      }}
    >
      {label || t('nodeLabelAdd')}
    </Text>
  );
}
