FROM node:20-alpine

WORKDIR /app

COPY db.json .
COPY server.js .

RUN npm install json-server

EXPOSE 3000

CMD ["node", "server.js"]
