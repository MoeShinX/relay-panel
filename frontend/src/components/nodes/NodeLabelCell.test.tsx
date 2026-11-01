import { describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { NodeLabelCell } from './NodeLabelCell';
import { NodeGroupSection } from './NodeGroupSection';

const t = ((key: string) => key) as unknown as Tfn;
const row = (over: Partial<NodeDisplayRow> = {}): NodeDisplayRow => ({ group_id: 1, group_name: 'g', node_id: 'n1', ...over });

/** The pencil is a button named by the tooltip text. */
const pencil = () => screen.getByRole('button', { name: 'nodeLabelEdit' });

describe('NodeLabelCell', () => {
  it('shows the label read-only for a user', () => {
    render(<NodeLabelCell row={row({ label: 'HK-1' })} t={t} />);
    expect(screen.getByText('HK-1')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'nodeLabelEdit' })).toBeNull();
  });

  it('shows a dash for a user when there is no label', () => {
    render(<NodeLabelCell row={row()} t={t} />);
    expect(screen.getByText('-')).toBeInTheDocument();
  });

  it('has nothing to name on a group\'s no-node placeholder row', () => {
    render(<NodeLabelCell row={row({ node_id: null })} t={t} onSave={vi.fn()} />);
    expect(screen.getByText('-')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'nodeLabelEdit' })).toBeNull();
  });

  it('offers an admin "add label" on an unnamed node, and saves what is typed', async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    const user = userEvent.setup();
    render(<NodeLabelCell row={row()} t={t} onSave={onSave} />);
    expect(screen.getByText('nodeLabelAdd')).toBeInTheDocument();

    await user.click(pencil());
    const box = await screen.findByRole('textbox');
    await user.type(box, '香港 HK-1');
    fireEvent.keyDown(box, { key: 'Enter', keyCode: 13 });
    fireEvent.keyUp(box, { key: 'Enter', keyCode: 13 });

    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ node_id: 'n1' }), '香港 HK-1');
  });

  it('starts from the current label, and an emptied one is sent as empty (clears it)', async () => {
    const onSave = vi.fn().mockResolvedValue(true);
    const user = userEvent.setup();
    render(<NodeLabelCell row={row({ label: 'HK-1' })} t={t} onSave={onSave} />);

    await user.click(pencil());
    const box = await screen.findByRole('textbox');
    expect(box).toHaveValue('HK-1');
    await user.clear(box);
    fireEvent.keyDown(box, { key: 'Enter', keyCode: 13 });
    fireEvent.keyUp(box, { key: 'Enter', keyCode: 13 });

    expect(onSave).toHaveBeenCalledWith(expect.anything(), '');
  });

  it('does not save when nothing changed', async () => {
    const onSave = vi.fn();
    const user = userEvent.setup();
    render(<NodeLabelCell row={row({ label: 'HK-1' })} t={t} onSave={onSave} />);
    await user.click(pencil());
    const box = await screen.findByRole('textbox');
    fireEvent.keyDown(box, { key: 'Enter', keyCode: 13 });
    fireEvent.keyUp(box, { key: 'Enter', keyCode: 13 });
    expect(onSave).not.toHaveBeenCalled();
  });

  it('stops typing at the length the server accepts', async () => {
    const user = userEvent.setup();
    render(<NodeLabelCell row={row()} t={t} onSave={vi.fn()} />);
    await user.click(pencil());
    const box = await screen.findByRole('textbox');
    expect(box).toHaveAttribute('maxlength', '40');
  });
});

describe('the label column on the node status page', () => {
  const props = { panelProtocol: 0, latestNodeVersion: '1.1.0', nodeVersionCheckFailed: false, isMobile: false, t, openDetail: vi.fn() };
  const header = () => Array.from(document.querySelectorAll('.ant-table-thead th')).map((th) => th.textContent);

  it('is not there while no node has a name, so the page looks as it always did', () => {
    render(<NodeGroupSection {...props} rows={[row({ online: true })]} />);
    expect(header()).not.toContain('nodeLabel');
  });

  it('appears, read-only, once some node has a name', () => {
    render(<NodeGroupSection {...props} rows={[row({ online: true, label: 'HK-1' })]} />);
    expect(header()).toContain('nodeLabel');
    expect(screen.getByText('HK-1')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'nodeLabelEdit' })).toBeNull();
  });

  it('puts the name on each mobile card too, and shows no "add" prompt on an unnamed one', () => {
    render(<NodeGroupSection {...props} isMobile rows={[row({ online: true, label: 'HK-1' }), row({ node_id: 'n2', online: true })]} />);
    expect(screen.getByText('HK-1')).toBeInTheDocument();
    expect(screen.queryByText('nodeLabelAdd')).toBeNull();
  });
});
