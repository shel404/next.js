/**
 * Based on https://github.com/facebook/react/blob/d4e78c42a94be027b4dc7ed2659a5fddfbf9bd4e/packages/react/src/ReactFetch.js
 */
import * as React from 'react'

const simpleCacheKey = '["GET",[],null,"follow",null,null,null,null]' // generateCacheKey(new Request('https://blank'));

function generateCacheKey(request: Request): string {
  // We pick the fields that goes into the key used to dedupe requests.
  // We don't include the `cache` field, because we end up using whatever
  // caching resulted from the first request.
  // Notably we currently don't consider non-standard (or future) options.
  // This might not be safe. TODO: warn for non-standard extensions differing.
  // IF YOU CHANGE THIS UPDATE THE simpleCacheKey ABOVE.
  return JSON.stringify([
    request.method,
    Array.from(request.headers.entries()),
    request.mode,
    request.redirect,
    request.credentials,
    request.referrer,
    request.referrerPolicy,
    request.integrity,
  ])
}

export function createDedupeFetch(originalFetch: typeof fetch) {
  const getCacheEntries = React.cache(
    // eslint-disable-next-line @typescript-eslint/no-unused-vars -- url is the cache key
    (url: string): Record<string, Promise<Response>> => ({})
  )

  return function dedupeFetch(
    resource: URL | RequestInfo,
    options?: RequestInit
  ): Promise<Response> {
    if (options && options.signal) {
      // If we're passed a signal, then we assume that
      // someone else controls the lifetime of this object and opts out of
      // caching. It's effectively the opt-out mechanism.
      // Ideally we should be able to check this on the Request but
      // it always gets initialized with its own signal so we don't
      // know if it's supposed to override - unless we also override the
      // Request constructor.
      return originalFetch(resource, options)
    }

    // Normalize the Request
    let url: string
    let cacheKey: string
    if (typeof resource === 'string' && !options) {
      // Fast path.
      cacheKey = simpleCacheKey
      url = resource
    } else {
      // Normalize the request.
      // if resource is not a string or a URL (its an instance of Request)
      // then do not instantiate a new Request but instead
      // reuse the request as to not disturb the body in the event it's a ReadableStream.
      const request =
        typeof resource === 'string' || resource instanceof URL
          ? new Request(resource, options)
          : resource
      if (
        (request.method !== 'GET' && request.method !== 'HEAD') ||
        // $FlowFixMe[prop-missing]: keepalive is real
        request.keepalive
      ) {
        // We currently don't dedupe requests that might have side-effects. Those
        // have to be explicitly cached. We assume that the request doesn't have a
        // body if it's GET or HEAD.
        // keepalive gets treated the same as if you passed a custom cache signal.
        return originalFetch(resource, options)
      }
      cacheKey = generateCacheKey(request)
      url = request.url
    }

    // Get the cache entries for the given URL.
    const cacheEntries = getCacheEntries(url)

    // Check if there is a cached entry for the given cache key. If there is, we
    // return the cached response (cloned). This will keep the cached promise to
    // remain unused and can be cloned on future requests.
    let promise = cacheEntries[cacheKey]
    if (promise) {
      return promise.then((response: Response) => response.clone())
    }

    // We pass the original arguments here in case normalizing the Request
    // doesn't include all the options in this environment.
    const original = originalFetch(resource, options)

    // We then clone the original response. We store this in the cache so that
    // any future requests will be using this cloned response.
    const cloned = original.then((response) => response.clone())

    // Attach an empty catch here so we don't get a "unhandled promise
    // rejection" warning
    cloned.catch(() => {})

    cacheEntries[cacheKey] = cloned

    // Return the promise so that the caller can await it. We pass back the
    // original promise.
    return original
  }
}
