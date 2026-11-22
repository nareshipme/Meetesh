import { useEffect, useState } from "react";
import { api, SearchResult } from "../api";

/** Search all meetings as the user types (debounced; a slow answer never overwrites a newer one). */
export function useSearch(query: string) {
  const [results, setResults] = useState<SearchResult[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults(null);
      setError(null);
      return;
    }
    let current = true;
    const timer = setTimeout(() => {
      api
        .searchMeetings(q)
        .then((r) => {
          if (!current) return;
          setResults(r);
          setError(null);
        })
        .catch((e) => current && setError(String(e)));
    }, 180);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [query]);

  return { results, error, searching: query.trim() !== "" && results === null && !error };
}
