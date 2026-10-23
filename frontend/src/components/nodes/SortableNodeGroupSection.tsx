import { useState } from 'react';
import type { ComponentProps, KeyboardEvent, MouseEvent, PointerEventHandler } from 'react';
import { HolderOutlined } from '@ant-design/icons';
import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { NodeGroupSection } from './NodeGroupSection';
import { useSuppressClickAfterDrag } from './useSuppressClickAfterDrag';

type SectionProps = Omit<ComponentProps<typeof NodeGroupSection>, 'dragHandle' | 'dragHeaderProps'>;

/** A node group the admin can drag to a new place in the group list. The whole
 *  header row is the grip (press and move a few pixels); a plain click on it
 *  still folds or unfolds the group. The keyboard way in is the handle. */
export function SortableNodeGroupSection({ id, ...props }: SectionProps & { id: number }) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id });

  const swallowClick = useSuppressClickAfterDrag(isDragging);
  // The whole header is the grip, so the handle is only the keyboard way in:
  // it takes no room (and shifts nothing on the header row) until it has focus.
  const [handleFocused, setHandleFocused] = useState(false);

  // Groups slide up or down to make room; they never move sideways or stretch.
  const style = {
    transform: CSS.Transform.toString(
      transform ? { ...transform, x: 0, scaleX: 1, scaleY: 1 } : null,
    ),
    transition,
    position: 'relative' as const,
    zIndex: isDragging ? 10 : undefined,
    opacity: isDragging ? 0.85 : undefined,
  };

  const label = props.t('dragToReorderGroup');
  const handle = (
    <span
      ref={setActivatorNodeRef}
      {...attributes}
      aria-label={label}
      title={label}
      data-testid={`group-drag-handle-${id}`}
      // Pointer drags start from the header row (see headerProps); the handle
      // only adds the keyboard way in. The header folds the group on a click
      // and on Enter; the handle must not: a click here is just a stray press.
      onClick={(e: MouseEvent) => e.stopPropagation()}
      onKeyDown={(e: KeyboardEvent<HTMLSpanElement>) => {
        // Space picks the group up (see the sensor in NodeStatus). Do NOT stop
        // its propagation: while a group is held, the arrow keys and the Space
        // that drops it are handled on `document`, and React stops a native
        // event from getting there when a handler stops it. Enter does nothing
        // on the handle, so keep it from reaching the header's fold toggle.
        listeners?.onKeyDown?.(e);
        if (e.key === 'Enter') e.stopPropagation();
      }}
      onFocus={() => setHandleFocused(true)}
      onBlur={() => setHandleFocused(false)}
      style={{
        cursor: 'grab',
        touchAction: 'none',
        color: 'var(--ant-color-text-tertiary, #999)',
        display: 'inline-flex',
        overflow: 'hidden',
        // 0 wide cancels the row's 8px gap too.
        width: handleFocused ? undefined : 0,
        marginRight: handleFocused ? 0 : -8,
      }}
    >
      <HolderOutlined />
    </span>
  );

  const headerProps = {
    // dnd-kit types its listener map as plain functions.
    onPointerDown: listeners?.onPointerDown as PointerEventHandler<HTMLDivElement> | undefined,
    // The click that follows a drop must not fold the group.
    onClick: (e: MouseEvent) => { if (swallowClick()) e.stopPropagation(); },
    style: { cursor: isDragging ? 'grabbing' : 'grab', userSelect: 'none' as const },
    'data-testid': `group-drag-header-${id}`,
    title: label,
  };

  return (
    <div ref={setNodeRef} style={style}>
      <NodeGroupSection {...props} dragHandle={handle} dragHeaderProps={headerProps} />
    </div>
  );
}
