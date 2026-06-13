import { Route, Routes } from 'react-router-dom';
import { ALL_ITEMS } from './nav';
import { PlaceholderPage } from '../pages/PlaceholderPage';

/**
 * Route table (设计系统 §5 / §6): one route per nav item (12 pages). Each
 * renders a navigable placeholder showing the page title + its primary API.
 * Page bodies are implemented later on this same base; an unknown path falls
 * back to the dashboard placeholder (fail-closed: no blank screen).
 */
export function AppRoutes() {
  return (
    <Routes>
      {ALL_ITEMS.map((item) => (
        <Route
          key={item.path}
          path={item.path}
          element={<PlaceholderPage title={item.label} api={item.api} />}
        />
      ))}
      <Route path="*" element={<PlaceholderPage title="未知页面" api="—" />} />
    </Routes>
  );
}
