import { describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { DataTable, clampPageSize, type Column } from './DataTable';

interface Row {
  id: string;
  n: number;
}

const columns: Column<Row>[] = [
  { key: 'id', header: 'id', render: (r) => r.id },
  { key: 'n', header: 'n', render: (r) => String(r.n), sortValue: (r) => r.n },
];

describe('clampPageSize (DB_PAGINATION_MANDATORY 缺省20钳200)', () => {
  it('defaults out-of-range NaN to 20', () => {
    expect(clampPageSize(Number.NaN)).toBe(20);
  });
  it('clamps above 200 down to 200', () => {
    expect(clampPageSize(5000)).toBe(200);
  });
  it('clamps below 1 up to 1', () => {
    expect(clampPageSize(0)).toBe(1);
    expect(clampPageSize(-10)).toBe(1);
  });
  it('passes a legal size through', () => {
    expect(clampPageSize(50)).toBe(50);
  });
});

describe('DataTable pagination', () => {
  const rows: Row[] = [
    { id: '1', n: 3 },
    { id: '2', n: 1 },
    { id: '3', n: 2 },
  ];

  it('renders the paged envelope total and current page', () => {
    render(
      <DataTable
        columns={columns}
        rows={rows}
        total={45}
        page={{ page_no: 1, page_size: 20 }}
        onPageChange={() => {}}
        rowKey={(r) => r.id}
      />,
    );
    // 45 total, size 20 ⇒ 3 pages.
    expect(screen.getByText(/共 45 条/)).toBeInTheDocument();
    expect(screen.getByText(/第 1\/3 页/)).toBeInTheDocument();
  });

  it('emits a clamped page size when the selector changes', () => {
    const onPageChange = vi.fn();
    render(
      <DataTable
        columns={columns}
        rows={rows}
        total={45}
        page={{ page_no: 2, page_size: 20 }}
        onPageChange={onPageChange}
        rowKey={(r) => r.id}
      />,
    );
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '200' } });
    expect(onPageChange).toHaveBeenCalledWith({ page_no: 1, page_size: 200 });
  });

  it('disables prev on the first page and next on the last', () => {
    render(
      <DataTable
        columns={columns}
        rows={rows}
        total={3}
        page={{ page_no: 1, page_size: 20 }}
        onPageChange={() => {}}
        rowKey={(r) => r.id}
      />,
    );
    expect(screen.getByText('上一页')).toBeDisabled();
    expect(screen.getByText('下一页')).toBeDisabled();
  });

  it('sorts within the current page when a sortable header is clicked', () => {
    render(
      <DataTable
        columns={columns}
        rows={rows}
        total={3}
        page={{ page_no: 1, page_size: 20 }}
        onPageChange={() => {}}
        rowKey={(r) => r.id}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /n/i }));
    const cells = screen.getAllByRole('cell');
    // After asc sort by n, first data row's n cell is "1".
    expect(cells[1]).toHaveTextContent('1');
  });

  it('shows the three states: loading skeleton then empty', () => {
    const { rerender } = render(
      <DataTable
        columns={columns}
        rows={[]}
        total={0}
        page={{ page_no: 1, page_size: 20 }}
        onPageChange={() => {}}
        rowKey={(r) => r.id}
        loading
      />,
    );
    expect(screen.getByRole('status')).toBeInTheDocument();

    rerender(
      <DataTable
        columns={columns}
        rows={[]}
        total={0}
        page={{ page_no: 1, page_size: 20 }}
        onPageChange={() => {}}
        rowKey={(r) => r.id}
        emptyTitle="空空如也"
      />,
    );
    expect(screen.getByText('空空如也')).toBeInTheDocument();
  });

  it('renders a fail-closed error state instead of rows', () => {
    render(
      <DataTable
        columns={columns}
        rows={rows}
        total={3}
        page={{ page_no: 1, page_size: 20 }}
        onPageChange={() => {}}
        rowKey={(r) => r.id}
        error={{ message: '坏了' }}
      />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('坏了');
    // No data leaked through the error state.
    expect(screen.queryByText('1')).not.toBeInTheDocument();
  });
});
