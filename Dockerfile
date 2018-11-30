FROM node:8

#RUN  apt-get -y update \
# && apt-get -y install python-pip python-dev build-essential \
# &&  pip  install locustio \

# Create app directory
WORKDIR /usr/local
# Bundle app source
COPY . .

EXPOSE 3000


CMD [ "URL=$URL node", "index.js" ]

#RUN locust -f locustfile.py --host=$URL -P 3000