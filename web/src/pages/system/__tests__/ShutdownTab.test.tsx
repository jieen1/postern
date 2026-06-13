import { describe, expect, it } from 'vitest';
import { screen, fireEvent, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { server } from '../../../mocks/server';
import { renderWithQuery } from './testUtils';
import { ShutdownTab } from '../ShutdownTab';

const BASE = '/v1';

describe('ShutdownTab', () => {
  it('requires the typed confirm word "shutdown" before the request fires', async () => {
    let shutdownCalled = false;
    server.use(
      http.post(`${BASE}/shutdown`, () => {
        shutdownCalled = true;
        return HttpResponse.json({ policy_rev: '4187' });
      }),
    );
    renderWithQuery(<ShutdownTab />);

    fireEvent.click(screen.getByText('关停 daemon'));
    const dialog = await screen.findByRole('dialog', { name: '确认：关停 daemon' });
    expect(dialog).toBeInTheDocument();

    // Confirm is disabled with no/wrong word.
    const confirmBtn = screen.getByText('关停', { selector: 'button' });
    expect(confirmBtn).toBeDisabled();
    fireEvent.click(confirmBtn);
    expect(shutdownCalled).toBe(false);

    // Type the exact word → confirm enables and fires.
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'shutdown' } });
    expect(confirmBtn).toBeEnabled();
    fireEvent.click(confirmBtn);
    await waitFor(() => expect(shutdownCalled).toBe(true));
    expect(await screen.findByText(/daemon 正在优雅关停/)).toBeInTheDocument();
  });

  it('on failure states explicitly "未关停" (fail-closed, stays running)', async () => {
    server.use(
      http.post(`${BASE}/shutdown`, () =>
        HttpResponse.json({ error: { code: 'denied', message: '无权关停' } }, { status: 403 }),
      ),
    );
    renderWithQuery(<ShutdownTab />);

    fireEvent.click(screen.getByText('关停 daemon'));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'shutdown' } });
    fireEvent.click(screen.getByText('关停', { selector: 'button' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('未关停');
    expect(alert).toHaveTextContent('无权关停');
    // No false "shutting down" status leaked.
    expect(screen.queryByText(/正在优雅关停/)).not.toBeInTheDocument();
  });
});
