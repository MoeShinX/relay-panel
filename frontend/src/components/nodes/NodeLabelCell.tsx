import { Typography } from 'antd';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';

const { Text } = Typography;

/** The longest label the panel accepts (characters), kept in step with
 *  `MAX_LABEL_CHARS` in the backend's node_labels service. */
export const MAX_NODE_LABEL = 40;

interface Props {
  row: NodeDisplayRow;
  t: Tfn;
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
export function NodeLabelCell({ row, t, onSave }: Props) {
  const label = row.label ?? '';
  if (!row.node_id) return <Text type="secondary">-</Text>;
  if (!onSave) {
    return label ? <Text ellipsis={{ tooltip: label }} style={{ maxWidth: 160 }}>{label}</Text> : <Text type="secondary">-</Text>;
  }
  return (
    <Text
      style={{ maxWidth: 170 }}
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
