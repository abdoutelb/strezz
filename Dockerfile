FROM node:8

RUN sudo apt-get update \
 && apt-get install python-pip python-dev build-essential \
 &&  pip install locustio \

# Create app directory
WORKDIR /usr/local/stress
# Bundle app source
COPY . .

EXPOSE 3000

RUN cd /usr/local/stress

CMD [ "URL=$0 node", "index.js" ]

RUN locust -f locustfile.py --host=$0 -P 3000