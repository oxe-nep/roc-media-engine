# roc-media-engine frontend (Next.js) – deploy to k3s.
# nginx proxies /api /ws /thumb /audio /hls to the capture-host engine (:8080).

FROM node:22-alpine AS builder
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .

# Empty backend URL + empty API key => browser uses same-origin paths.
# nginx injects X-API-Key from runtime env API_KEY (k3s Secret).
ARG NEXT_PUBLIC_BACKEND_URL=
ARG NEXT_PUBLIC_API_KEY=
ENV NEXT_PUBLIC_BACKEND_URL=$NEXT_PUBLIC_BACKEND_URL
ENV NEXT_PUBLIC_API_KEY=$NEXT_PUBLIC_API_KEY

RUN npm run build

FROM node:22-alpine
WORKDIR /app

RUN apk add --no-cache nginx gettext \
  && mkdir -p /etc/nginx/templates /run/nginx \
  && rm -f /etc/nginx/http.d/default.conf

COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
COPY --from=builder /app/public ./public
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh

ENV BACKEND_HOST=10.199.28.249
ENV BACKEND_PORT=8080
ENV API_KEY=change-me
ENV NODE_ENV=production

EXPOSE 80
CMD ["/docker-entrypoint.sh"]
